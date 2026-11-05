# 安装、更新与卸载

WW-StudyApp 支持 Windows、Linux 和 macOS。请从发布页面选择对应系统的安装包。

Windows 10/11 x64 提供安装版与便携版，均使用当前用户权限。Markdown 编辑器需要 Microsoft Edge WebView2 Runtime。

## 选择安装包

从 [GitHub Releases](https://github.com/xwwpy/WW-StudyAPP/releases) 下载：

| 文件 | 使用方式 |
| --- | --- |
| `WW-StudyApp-<版本>-installer-x64.zip` | 解压后双击 `INSTALL-WW-STUDYAPP.cmd`。推荐使用：脚本会把本次安装的临时目录放到包内，避免系统临时目录不可写导致安装失败。 |
| `WW-StudyApp-<版本>-windows-x86_64-Setup.exe` | 直接双击安装。如果提示无法写入临时文件，请改用上面的 ZIP。 |
| `WW-StudyApp-<版本>-portable-x64.zip` | 完整解压后双击 `START-WW-STUDYAPP.cmd`。临时目录与 WebView2 数据留在包内，便于搬移整个文件夹；也可以直接运行 `WW-StudyApp.exe`。 |

发布页同时提供 `SHA256SUMS.txt`，列出发布文件的 SHA-256 校验值；deb 和 dmg 另有同名 `.sha256` 文件。

安装时可在「选择安装位置」页面输入路径或浏览文件夹，支持中文和空格。默认位置为 `%LOCALAPPDATA%\Programs\WW-StudyApp`，请选择当前用户有写入权限的目录。

如果已经安装，重装和更新会沿用原目录。如需更换位置，请先卸载已有版本，再运行安装程序选择新目录；卸载会保留学习资料。

安装后桌面会出现带应用图标的「WW-StudyApp」快捷方式，开始菜单同时提供应用与卸载入口。完成页可勾选「运行 WW-StudyApp」立即打开应用。

使用 ZIP 中的安装脚本时，本次安装的临时文件会保留在包内 `temp` 文件夹；安装结束并关闭安装程序后，可自行清理该文件夹。

## Linux 安装


支持 Ubuntu 24.04 及兼容系统的 x86_64 桌面，需要 X11 或 XWayland，以及可用的 Vulkan 驱动。下载 `.deb` 和同名 `.sha256` 文件，在下载目录运行：

```bash
sha256sum --check WW-StudyApp-0.2.5-linux-x86_64.deb.sha256
sudo apt install ./WW-StudyApp-0.2.5-linux-x86_64.deb
studyapp
```

安装后也可从应用菜单打开 WW-StudyApp。更新时下载新版本的 `.deb`，再次执行安装命令；笔记与设置会保留。WebDAV 凭据通过桌面密钥环保存，使用前请确保登录密钥环已解锁。

## macOS 安装

需要 macOS 15 或更高版本。Apple 芯片选择 `macos-arm64.dmg`，Intel 芯片选择 `macos-x86_64.dmg`。打开磁盘映像，将 WW-StudyApp 拖到 Applications 后启动。

也可在下载目录使用命令安装（以下为 Apple 芯片版本）：

```bash
shasum -a 256 --check WW-StudyApp-0.2.5-macos-arm64.dmg.sha256
hdiutil attach WW-StudyApp-0.2.5-macos-arm64.dmg
sudo ditto /Volumes/WW-StudyApp/WW-StudyApp.app /Applications/WW-StudyApp.app
hdiutil detach /Volumes/WW-StudyApp
open /Applications/WW-StudyApp.app
```

当前 macOS 安装包采用临时签名，尚未经过 Apple 公证。若系统阻止启动，请先确认下载来源和校验结果，再到“系统设置 → 隐私与安全性”中选择“仍要打开”。手动更新时，退出应用后将新版本复制到 Applications 替换旧版本。

## 检查与安装更新

- 每次正常启动后检查一次新版本，运行期间每小时检查；也可以在「设置 → 关于 / 账户」中点击「检查更新」。
- 发现新版本后，侧栏和关于页的当前版本旁显示未读红点，不会自动弹出整篇日志。点击版本入口或「查看更新内容」打开更新卡片；已安装版本之后、目标版本以内的更新说明按版本从新到旧分别显示。阅读后红点消失，仍可再次打开卡片安装更新；出现后续新版本时再次标记未读。日志可选中复制，长内容在同一个区域滚动，更新与关闭按钮保持可见。
- 点「立即更新」后，应用会下载安装包并校验发布签名，处理尚未保存的设置并保存草稿，再退出安装，完成后重新打开原资料库。需要系统授权时请按提示操作。
- 更新包只从本仓库的 Releases 下载，签名校验失败时不会运行安装程序，并会显示失败原因。
- Windows 0.2.4 起，即使误以管理员身份启动，更新也会交给同一用户的普通权限进程，安装和重启不继续保留管理员权限。请仍选择当前用户可写的安装目录。0.2.3 及更早版本若提示先正常重启，请先退出并直接双击应用（不要选择「以管理员身份运行」），再更新或手动安装新版。
- 更新不会清空学习资料。仍建议在更新前[导出备份](storage.md)。

应用不会在未确认的情况下自行安装更新。更新检查会使用系统代理配置，遇到临时连接失败会自动重试一次。如果网络不可用或下载失败，可以稍后点击「检查更新」或「重试更新」，并继续使用当前版本。

应用内自动安装适用于 Windows 安装版、完整解压的 Windows 便携版、通过 deb 安装的 Linux，以及安装到本机文件夹中的 macOS 应用。Windows 安装版沿用原安装目录；便携版原位更新并重新打开，不创建卸载项或更改另一份安装版的快捷方式。Windows 便携版 0.2.2 及更早版本需先退出并手动完整解压 0.2.3，之后可使用应用内更新；macOS 请勿从磁盘映像中运行更新。Linux/macOS 的 0.1.2 用户需先手动安装 0.1.3，再使用后续版本的应用内更新。

macOS 安装包尚未经过 Apple 公证，首次打开可能需要按系统提示正常确认。应用内更新经过发布签名验证和系统启动检查；若系统阻止新版本，当前应用和未保存内容会保留，请确认下载来源后退出应用并手动安装新版。

macOS 如提示当前目录不支持自动更新，请将应用放入本人可写的 `~/Applications`，或退出应用后手动替换为新版本。

## 卸载

从 Windows「设置 → 应用 → 已安装的应用」或开始菜单的「卸载 WW-StudyApp」运行卸载程序。卸载只删除所选安装目录内的程序文件、桌面和开始菜单快捷方式及卸载记录，配置与资料库（`%APPDATA%\StudyApp`）会保留。

需要彻底清理时，卸载完成后手动删除 `%APPDATA%\StudyApp`。原始 Markdown 笔记、图片和导出备份不在安装目录内，卸载不会删除它们。

## 从其他发行版本迁移

如果你之前安装的应用使用其他发布仓库，请手动安装本仓库的版本：

1. 先导出备份并退出应用，在 Windows「设置 → 应用」中卸载旧版；卸载默认保留配置和学习资料。
2. 下载最新安装包并运行 `INSTALL-WW-STUDYAPP.cmd`（或直接运行 `Setup.exe`）。
3. 启动应用并确认资料库位置。本仓库从 0.1.0 开始发布，后续版本可在应用内检查、下载并安装。
