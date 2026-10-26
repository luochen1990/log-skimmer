# 职责: devShell — 本地开发环境入口 (公开贡献者可用; 系统无 rust 工具链时
# 经 `nix develop -c just check` 进入本环境)。只提供 devShells output,
# 不做 package/checks (构建走 cargo, 质量循环走 justfile)。
# wasm 形态 (log-skimmer-wasm) 开箱即用: nixpkgs rustc 的 sysroot 自带
# wasm32-unknown-unknown std (构建期无条件编译该目标), 唯一缺口是链接器 —
# nixpkgs rustc 以 --disable-lld 构建 (宿主链接用 stdenv cc), wasm 目标链接
# 回退查 PATH 上的 lld (rustup 发行版则自带 rust-lld), 故 devShell 补 lld。
{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      devShells = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              # rust 工具链 (nixpkgs stable; sysroot 含 wasm32-unknown-unknown std)
              rustc
              cargo
              clippy
              rustfmt
              lld # wasm32 目标链接器 (just wasm-check; 见文件头注释)
              nodejs # wasm 对账宿主 (just wasm-check; 标准 WebAssembly API, 零 npm 依赖)
              # 开发辅助: 任务入口 + 日志/规则集调试工具链
              just # 任务入口 (check/rules-load/wasm-check)
              jq # JSON 处理 (skim --json 输出消费)
              ripgrep # rg 基线对照与开发调试
              yq # YAML 处理储备 (规则集工具链)
            ];
            # 宿主 rustup 剥离 (spike 教训): cargo 对外部子命令 (clippy/fmt) 的
            # 查找含 $CARGO_HOME/bin — 宿主 rustup 的 ~/.cargo/bin shim (默认
            # 工具链 1.79, 读不了 Cargo.lock v4) 会抢占 devShell 的 nix 工具链,
            # 即使它不在 PATH 上。隔离 CARGO_HOME 即彻底切断; registry 缓存随之
            # 独立 (首次构建经 rsproxy 重下, 一次性成本)。
            shellHook = ''
              export CARGO_HOME="''${XDG_CACHE_HOME:-$HOME/.cache}/log-skimmer/cargo-home"
            '';
          };
        });
    };
}
