# WW-StudyAPP

本地优先的学习笔记应用，支持 Windows、Linux 和 macOS。

- Markdown 笔记编辑、文件夹管理、搜索与自动保存。
- 学习记录、定时提醒、间隔复习与学习统计。
- 图片管理、备份恢复，以及 WebDAV、本地文件夹和 NAS 同步。
- 主题与快捷键设置；应用内显示 Markdown 更新日志并提示新版本。

从 [Releases](https://github.com/xwwpy/WW-StudyAPP/releases) 下载对应系统的安装包。详细操作见[使用指南](docs/user-guide.md)。

## Linux 安装

支持 Ubuntu 24.04 及兼容系统的 x86_64 桌面，需要 X11 或 XWayland，以及可用的 Vulkan 驱动。下载 `.deb` 和同名 `.sha256` 文件，在下载目录运行：

```bash
sha256sum --check WW-StudyApp-0.1.3-linux-x86_64.deb.sha256
sudo apt install ./WW-StudyApp-0.1.3-linux-x86_64.deb
studyapp
```

安装后也可从应用菜单打开 WW-StudyApp。更新时下载新版本的 `.deb`，再次执行安装命令；笔记与设置会保留。WebDAV 凭据通过桌面密钥环保存，使用前请确保登录密钥环已解锁。

## macOS 安装

需要 macOS 15 或更高版本。Apple 芯片选择 `macos-arm64.dmg`，Intel 芯片选择 `macos-x86_64.dmg`。打开磁盘映像，将 WW-StudyApp 拖到 Applications 后启动。

也可在下载目录使用命令安装（以下为 Apple 芯片版本）：

```bash
shasum -a 256 --check WW-StudyApp-0.1.3-macos-arm64.dmg.sha256
hdiutil attach WW-StudyApp-0.1.3-macos-arm64.dmg
sudo ditto /Volumes/WW-StudyApp/WW-StudyApp.app /Applications/WW-StudyApp.app
hdiutil detach /Volumes/WW-StudyApp
open /Applications/WW-StudyApp.app
```

当前 macOS 安装包采用临时签名，尚未经过 Apple 公证。若系统阻止启动，请先确认下载来源和校验结果，再到“系统设置 → 隐私与安全性”中选择“仍要打开”。手动更新时，退出应用后将新版本复制到 Applications 替换旧版本。

## 应用内更新

启动时自动检查一次，运行期间每小时检查。发现新版本后会显示更新卡片，点击“立即更新”下载并验证安装包，保存当前草稿后安装并重新打开应用；需要权限时请完成系统授权。点击“稍后再说”后，同一版本在本次运行中不再主动弹出，也可以从设置中手动检查。

Windows 需要使用安装版，Linux 需要通过 deb 安装；macOS 请先将应用安装到本机文件夹并退出磁盘映像中的副本。macOS 首次打开可能需要按系统提示正常确认；应用内更新经过发布签名验证和系统启动检查。若系统阻止新版本，当前应用会保持打开，请退出后手动安装新版。从 0.1.2 升级的 Linux/macOS 用户需先手动安装 0.1.3，此后的版本可使用应用内更新。

macOS 如提示当前目录不支持自动更新，请将应用放入本人可写的 `~/Applications`，或退出应用后手动替换为新版本。
