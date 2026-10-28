# WW-StudyApp 0.1.2

- 新增 Linux x86_64 `.deb` 安装包，支持 Ubuntu 24.04 及兼容系统，在 X11/XWayland 桌面运行。
- 新增 macOS 15 及以上的 `.dmg` 安装包，分别适用于 Apple Silicon 和 Intel Mac。
- Linux 和 macOS 支持连续 Markdown 笔记编辑、摘要编写与复习；同步密码和图床密钥保存到系统钥匙串。
- README 新增 Linux、macOS 的下载与安装命令。更新时下载安装对应平台的安装包，已有资料库会保留。

Linux 使用 `sudo apt install ./WW-StudyApp-0.1.2-linux-x86_64.deb` 安装。macOS 打开对应芯片的 DMG，将 WW-StudyApp 拖入「应用程序」。macOS 安装包未经过 Apple 公证，首次打开可能需要在「系统设置 → 隐私与安全性」允许打开。Linux 保存远程同步或图床凭据需要已解锁的 Secret Service（如 GNOME Keyring）。

Windows 用户继续使用 Setup 安装包或便携包。各平台安装文件的校验值见 `SHA256SUMS.txt`。
