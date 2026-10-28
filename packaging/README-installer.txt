WW-StudyApp 安装说明
====================

请双击 INSTALL-WW-STUDYAPP.cmd，不要直接双击 Setup.exe。

原因：如果 Windows 的 TEMP / TMP 指向不可写目录（例如 E:\Temp），直接双击
安装器会提示：

    NSIS Error
    Error writing temporary file. Make sure your temp folder is valid.

脚本会把本次安装使用的临时目录切换到本文件夹下的 temp 目录。安装结束并关闭
安装程序后，可以自行清理该目录。

安装位置可在安装向导中选择，支持中文和空格；请选择当前用户可写入的文件夹。
默认位置：%LOCALAPPDATA%\Programs\WW-StudyApp（无需管理员权限）。
之后的应用内更新会沿用所选目录。
已有安装如需换位置，请先卸载再重新安装；学习资料会保留。
桌面会创建带应用图标的 WW-StudyApp 快捷方式，开始菜单提供应用与卸载入口。

资料库与配置位于 %APPDATA%\StudyApp\StudyApp；卸载默认保留，只有你在卸载后
手动删除或使用应用的清理功能时才会移除。

系统要求：Windows 10 / 11 x64，Microsoft Edge WebView2 Runtime。
安装完成后，应用可以自动检查 GitHub Releases 上的新版本。
