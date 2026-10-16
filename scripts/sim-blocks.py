#!/usr/bin/env python3
# sim-blocks.py — 状态机块边界模拟器 (Log Skimmer POC, 引擎无关自验)
#
# 职责边界: 按 docs/rule-format.md A2/A3 语义 (锚开块 / 扩展指针 p 有序透传 / loop 贪心 /
# 越界闭块) 模拟单条规则对单个日志的块提取, 打印各 block 的 1-based 行区间与首末行内容 —
# 用于在引擎落地前人工核对块边界是否符合规则意图。仅支持本仓规则的固定 YAML 子集
# (id/patterns[].regex/loop), 非通用 YAML 解析器。无 LLM、无网络。
import re
import sys


def parse_rule(path):
    id_, regexes, loops = None, [], []
    for line in open(path, encoding="utf-8"):
        if id_ is None:
            m = re.match(r"^id: (.+)$", line)
            if m:
                id_ = m.group(1)
            continue
        m = re.match(r"^\s*-\s*regex: '(.*)'\s*$", line)
        if m:
            regexes.append(m.group(1).replace("''", "'"))
            loops.append(False)
            continue
        m = re.match(r"^\s*loop: true\s*$", line)
        if m and regexes:
            loops[-1] = True
    return id_, regexes, loops


def scan(regexes, loops, lines):
    comp = [re.compile(r) for r in regexes]
    blocks, i, n = [], 0, len(lines)
    while i < n:
        if comp[0].search(lines[i]):
            start, p, i = i, 1, i + 1
            while i < n and p < len(comp):
                if comp[p].search(lines[i]):
                    if not loops[p]:
                        p += 1
                    i += 1
                else:
                    p += 1  # 透传: 越过当前 pattern, 同一行继续试下一个
            blocks.append((start + 1, i))  # i = 闭块行 (未消费) 或 n+1 (EOF)
        else:
            i += 1
    return blocks


def main():
    rule_path, log_path = sys.argv[1], sys.argv[2]
    id_, regexes, loops = parse_rule(rule_path)
    lines = [l.rstrip("\r\n") for l in open(log_path, encoding="utf-8", errors="replace")]
    blocks = scan(regexes, loops, lines)
    print(f"rule={id_} file={log_path.split('/')[-1]} blocks={len(blocks)}")
    for s, e in blocks:  # s=1-based 首行; e=0-based 闭块行索引 (= 1-based 末消费行)
        first = lines[s - 1][:100]
        last = lines[e - 1][:100] if e > s else ""
        print(f"  [L{s}-L{e}] ({e - s + 1} 行) 首: {first!r}")
        print(f"          末: {last!r}")


if __name__ == "__main__":
    main()
