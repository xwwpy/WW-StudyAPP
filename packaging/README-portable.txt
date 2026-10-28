WW-StudyApp 便携版说明
======================

双击 START-WW-STUDYAPP.cmd 启动。

脚本会把临时目录（TEMP/TMP）和 WebView2 用户数据目录放到本文件夹内，便于
整个文件夹搬移或放在 U 盘中使用。也可以直接运行 WW-StudyApp.exe，此时会使用
系统临时目录和默认的 WebView2 数据目录。

系统要求：Windows 10 / 11 x64，Microsoft Edge WebView2 Runtime。

资料库与配置默认位于 %APPDATA%\StudyApp\StudyApp；如果希望完全便携，可以在
「设置 → 存储」中把资料库指向本文件夹下的位置。

便携版不写入“应用”列表，也不需要卸载：退出应用后删除整个文件夹即可。
