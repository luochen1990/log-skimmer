# 职责: devShell — 本地与 CI (vm-nix runner) 统一的开发环境入口。
# 只提供 devShells output, 不做 package/checks (M1 阶段构建走 cargo, CI 门禁走 just ci-merge);
# CI runner 无系统 cargo, 门禁链经 `nix develop --command just _ci-merge` 进入本环境。
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
              # 门禁链与脚本依赖
              just # 任务入口 (ci-merge 契约宿主)
              jq # l0-collect/summarize/gate 全链
              ripgrep # rg 基线对照与开发调试
              yq # YAML 处理储备 (规则集工具链)
            ];
            # crates 镜像 SSOT 在 .cargo/config.toml (source replacement, 本地/devShell/CI 同源)
          };
        });
    };
}
