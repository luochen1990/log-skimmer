// log-skimmer-wasm 宿主参考实现 + wasm↔原生 CLI 对账 (just wasm-check 消费)。
//
// 职责: ① 宿主编排示范 (未来 web 仓直接移植): 规则目录收集 → ABI 三段入参 →
//   4B 长度前缀 envelope 解析 (收集与入参的踩坑注释见各使用点);
// ② 与原生 CLI --json 同样本全量对账 (blocks 深比较含 fragments/collapse 标记
//   + stats + input/output_lines);
// ③ 可选 --bench 计时 (预热 1 + 计时 3 取中位)。
//
// 用法:
//   node reconcile.mjs <rules_dir> <log_file> --native <cli_json>   # 对账 (不一致 exit 1)
//   node reconcile.mjs <rules_dir> <log_file> [--bench]             # 冒烟 + 可选计时
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { performance } from 'node:perf_hooks';

const [,, rulesDir, logFile, ...rest] = process.argv;
const nativePath = rest.includes('--native') ? rest[rest.indexOf('--native') + 1] : null;
const bench = rest.includes('--bench');
if (!rulesDir || !logFile) {
  console.error('usage: node reconcile.mjs <rules_dir> <log_file> [--native <cli_json>] [--bench]');
  process.exit(2);
}

// ---- 规则收集 (镜像 engine collect_yaml_files 的目录遍历语义; manifest.yaml
// 照收 — 保留名识别与版本协商在引擎侧单点) ----
function collectYaml(dir, out) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (e.name.startsWith('.')) continue; // 隐藏路径 = 仓级元数据, 非规则
    const p = path.join(dir, e.name);
    if (e.isDirectory()) collectYaml(p, out);
    else if (e.name.endsWith('.yaml') || e.name.endsWith('.yml')) out.push(p);
  }
}
// 路径序比较器 — 镜像 Rust Path 的 Ord (逐组件比较, 组件前缀短者排前:
// 'a/b.yaml' < 'a.yaml'), 整串字节序在此形态下会翻转 ('.' 0x2E < '/' 0x2F)。
function cmpPath(a, b) {
  const A = a.split(path.sep), B = b.split(path.sep);
  for (let i = 0; i < Math.min(A.length, B.length); i++) {
    if (A[i] !== B[i]) return A[i] < B[i] ? -1 : 1;
  }
  return A.length - B.length;
}
const ruleFiles = [];
collectYaml(rulesDir, ruleFiles);
ruleFiles.sort(cmpPath); // 与 Rust 侧 files.sort() 等序 (Path Ord 组件序)
const sources = ruleFiles.map(p => [path.relative(rulesDir, p), fs.readFileSync(p, 'utf8')]);
const rulesJson = JSON.stringify(sources);

// ---- 日志读取 (gz 透明解压, 与 sample_io::read_log_sample 对齐) ----
let logBuf = fs.readFileSync(logFile);
if (logFile.endsWith('.gz')) logBuf = zlib.gunzipSync(logBuf);
const logText = new TextDecoder('utf-8').decode(logBuf);
const fileName = path.basename(logFile);

// ---- wasm 实例化 (产物路径相对本脚本定位, 与 cwd 无关) ----
const wasmPath = new URL('../target/wasm32-unknown-unknown/release/log_skimmer_wasm.wasm', import.meta.url);
const wasmBytes = fs.readFileSync(wasmPath);
const { instance } = await WebAssembly.instantiate(wasmBytes, {});
const ex = instance.exports;
const enc = new TextEncoder();
const dec = new TextDecoder('utf-8');

// alloc 三段入参 (可能触发 memory 增长 detach 旧视图) → 全部 alloc 完再统一写入。
// 注意: 须按 UTF-8 字节长 alloc (JS string.length 是 UTF-16 单位数, 非 ASCII 时
// 小于 UTF-8 字节长, 越界写会破坏堆)。
function sendAll(strs) {
  const bufs = strs.map(s => enc.encode(s));
  const ptrs = bufs.map(b => ex.wasm_alloc(b.length));
  const views = ptrs.map((p, i) => new Uint8Array(ex.memory.buffer, p, bufs[i].length));
  views.forEach((v, i) => v.set(bufs[i]));
  return ptrs.map((p, i) => ({ ptr: p, len: bufs[i].length }));
}
function callSkim() {
  const [r, f, l] = sendAll([rulesJson, fileName, logText]);
  const outPtr = ex.wasm_skim_json(r.ptr, r.len, f.ptr, f.len, l.ptr, l.len);
  for (const b of [r, f, l]) ex.wasm_free(b.ptr, b.len);
  const mem = new Uint8Array(ex.memory.buffer);
  const payloadLen = mem[outPtr] | (mem[outPtr + 1] << 8) | (mem[outPtr + 2] << 16) | (mem[outPtr + 3] << 24);
  const payload = dec.decode(mem.subarray(outPtr + 4, outPtr + 4 + payloadLen));
  ex.wasm_free(outPtr, 4 + payloadLen);
  return JSON.parse(payload);
}

// ---- 冒烟 ----
const env = callSkim();
if (!env.ok) {
  console.error('skim 失败:', env.error);
  process.exit(1);
}
const res = env.result;
console.log(`样本: ${fileName}`);
console.log(`输入行数: ${res.input_lines}, 输出行数: ${res.output_lines}, blocks: ${res.blocks.length}`);
const first = res.blocks[0], last = res.blocks[res.blocks.length - 1];
console.log(`首块: L${first.start_line}-${first.end_line} rules=[${first.rules.join(',')}]`);
console.log(`尾块: L${last.start_line}-${last.end_line} rules=[${last.rules.join(',')}]`);

// ---- 对账 (与原生 CLI --json) ----
if (nativePath) {
  const native = JSON.parse(fs.readFileSync(nativePath, 'utf8'));
  let ok = true;
  const cmp = (label, a, b) => { if (a !== b) { ok = false; console.error(`MISMATCH ${label}: wasm=${a} native=${b}`); } };
  cmp('input_lines', res.input_lines, native.input_lines);
  cmp('output_lines', res.output_lines, native.output_lines);
  cmp('blocks.length', res.blocks.length, native.blocks.length);
  if (res.blocks.length === native.blocks.length) {
    for (let i = 0; i < res.blocks.length; i++) {
      const w = res.blocks[i], n = native.blocks[i];
      cmp(`blocks[${i}].start_line`, w.start_line, n.start_line);
      cmp(`blocks[${i}].end_line`, w.end_line, n.end_line);
      cmp(`blocks[${i}].rules`, w.rules.join('|'), n.rules.join('|'));
      // 深比较 (逐块整体 stringify): fragments 全字段 + collapse 标记 全覆盖 —
      // 两侧同源 serde 序列化, 键序恒一致, stringify 逐字节可比。
      cmp(`blocks[${i}] (deep)`, JSON.stringify(w), JSON.stringify(n));
    }
  }
  cmp('stats', JSON.stringify(res.stats), JSON.stringify(native.stats));
  console.log(ok ? '对账通过: wasm 与原生 CLI 全量一致 (blocks/stats)' : '对账失败');
  if (!ok) process.exit(1);
}

// ---- 基准: 预热 1 + 计时 3 取中位 ----
if (bench) {
  callSkim(); // 预热
  const times = [];
  for (let i = 0; i < 3; i++) {
    const t0 = performance.now();
    const r = callSkim();
    const t1 = performance.now();
    if (!r.ok) throw new Error('bench 调用失败: ' + r.error);
    times.push(t1 - t0);
  }
  times.sort((a, b) => a - b);
  console.log(`bench (ms): ${times.map(t => t.toFixed(1)).join(' / ')} — 中位 ${times[1].toFixed(1)}`);
}
