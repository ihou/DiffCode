# Mac App Store 版发布

App Store 版和官网下载版是同一套代码（`DifKit`），入口不同：`DifAppStore` 不链接 Sparkle，运行在 App Sandbox 里。这个版本目前没有上架；下面是它的构建方法。

## 和官网版的区别

| | 官网版 | App Store 版 |
| --- | --- | --- |
| 更新 | Sparkle（`release/macos/`） | App Store |
| 沙盒 | 否 | 是：只能访问用户选择过的文件夹 |
| 内置终端 | 有 | 没有（沙盒里的 shell 用不了用户的工具和文件，也有审核风险） |
| `dif` 命令 | 有 | 没有 |
| Git | 系统的 git | 同样是系统的 git，但必须直接调用二进制（见下） |
| 网络 | 不限 | 只允许出站连接（`network.client`），给 fetch / pull / push 用；SSH 密钥要在授权个人文件夹后才读得到 |

**文件夹访问**：添加项目（打开面板、拖进窗口、在 Finder 里用 DiffCode 打开）时保存 security-scoped bookmark，之后每次启动自动恢复。授权一个文件夹就覆盖其中所有仓库。文件 ▸ 授权访问文件夹… 默认指向个人文件夹：授权后 git 能读到 `~/.gitconfig`（提交者身份、全局忽略规则）。失去授权的项目会留在列表里，显示「授权访问…」按钮。

**Git**：沙盒里 `/usr/bin/git` 和 `xcrun` 都拒绝运行，所以依次找 Homebrew、`xcode-select` 指向的开发者目录、Command Line Tools、Xcode 里的 git 二进制；都没有时显示安装说明（`xcode-select --install`）。git 作为子进程继承应用的沙盒和授权。

> 审核风险：App Store 版依赖用户装有 Command Line Tools 或 Xcode。审核员的机器若没有，会看到安装提示页。审核备注里应写明这是给开发者用的 Git 工具，需要先装 Xcode 或 Command Line Tools。若因此被拒，可以改为内置 git（GPLv2，需评估许可）或改用 libgit2（工作量大）。

## 发布步骤

前提：账号持有人已在 App Store Connect ▸ 协议、税务和银行业务 里接受最新协议（否则 API 和公证都返回 403 `REQUIRED_AGREEMENTS_MISSING_OR_EXPIRED`）。

1. `release/appstore/.appstore.env`：从 `.appstore.env.example` 复制，填团队、bundle ID 和 App Store Connect API Key 的路径。文件被 git 忽略。
2. `release/appstore/scripts/setup_signing.sh`（先 `DRY_RUN=1` 看一遍）。可以重复运行，已有的就跳过；证书过期后再跑一次就会续上。它会：
   - 注册 bundle ID `com.nanshanbei.DiffCode`（macOS）；
   - 钥匙串里没有有效的 Mac Installer Distribution 或 Apple Distribution 证书时，本机生成私钥和 CSR 向 Apple 申请，并导入登录钥匙串（私钥不离开这台 Mac）；
   - 复用对得上的 Mac App Store 描述文件；过期的、或者证书已经换掉的就删掉重建。存到 `release/appstore/profiles/`（已忽略）。
3. 在 App Store Connect 网页上新建 App：平台 macOS，bundle ID `com.nanshanbei.DiffCode`（API 不能新建 App）。
4. 在 `release/version.env` 里调大版本号/构建号，然后 `./build_appstore.sh`：构建通用二进制，用有效的 Apple Distribution 证书签名（按 SHA-1 选定，钥匙串里有已吊销的同名证书也不会选错），嵌入描述文件，检查沙盒权限，打成安装包，`altool --validate-app` 预检。预检要对照第 3 步建的 App；还没建时打完包就停下，并提示去建（`VALIDATE=0` 只打包）。
5. `./upload_appstore.sh`：上传。处理完后在 App Store Connect 里选择这个构建；**脚本不会提交审核**。

商店文案、截图和提交审核在 App Store Connect 里完成。

## 证书有效期

Apple Distribution 证书（给 App 签名，描述文件跟着它一起到期）和 Mac Installer Distribution 证书（给安装包签名）都会过期。到期后 `./build_appstore.sh` 会报找不到有效证书，或描述文件过期，重新跑一次 `setup_signing.sh` 就行。
