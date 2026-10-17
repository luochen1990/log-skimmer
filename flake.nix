# 职责: devShell — 本地开发环境入口 (公开贡献者可用; 系统无 rust 工具链时
# 经 `nix develop -c just check` 进入本环境)。只提供 devShells output,
# 不做 package/checks (构建走 cargo, 质量循环走 justfile)。
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
              # rust 工具链 (nixpkgs stable)
              rustc
              cargo
              clippy
              rustfmt
              # 开发辅助: 任务入口 + 日志/规则集调试工具链
              just # 任务入口 (check/rules-load)
              jq # JSON 处理 (skim --json 输出消费)
              ripgrep # rg 基线对照与开发调试
              yq # YAML 处理储备 (规则集工具链)
            ];
          };
        });
    };
}
