# BastionTerm · 垒桥终端

**让 AI Agent 在只开放交互式 SSH 的堡垒机上工作，你可以看着它执行，也可以随时接手。**

BastionTerm gives your agent and your terminal the same SSH shell. It works with bastions that allow an interactive PTY but disable SSH exec channels. Open a host tab, follow the output, and take over when a command needs your input. Agents connect over MCP.

普通 Agent 常用 `ssh host command`。有的堡垒机允许用户登录，却拒绝这种远程执行请求。垒桥终端沿用你的账号和访问权限，在已登录的交互式 shell 中发送命令并读取结果。同一台主机保留工作目录和环境，不同主机独立接管、交还。

**和 Warp 比，哪里更顺手？** 在我们日常使用的交互式堡垒机上，最省事的是交接。Agent 执行到一半需要人工处理时，你在同一标签里接手，处理完再交还，双方都能看到刚才发生了什么。

[下载 macOS 或 Windows 版本](https://github.com/jichengwei001/bastionterm/releases) · [MCP 配置与 Agent 提示词](MCP配置与Agent提示词.md)

## 三步开始

1. 解压对应平台的发布包，打开 BastionTerm，点「连接配置」添加你有权访问的 SSH 主机。
2. 登录后点「MCP 设置」，把生成的配置合并到 Agent 客户端，重新加载客户端。
3. 告诉 Agent「在刚配置的主机运行 `pwd`」。你会看到实时输出，点「接管终端」即可操作同一个 shell。

## 使用桌面终端

macOS 解压后打开完整的 BastionTerm.app。Windows 原生构建生成一个 BastionTerm.exe。顶部的「连接配置」或「＋」可以新建、修改、删除配置或连接主机。连接配置中可用 ↑ 和 ↓ 调整端口，按主机名称的开头可在列表中查找配置。保存密码后可留空沿用。改变主机、账号或端口后需重新输入密码。取消保存配置可以只连接一次。

启动窗口会显示后台连接状态。若提示后台启动失败，可查看数据目录中的 `service.log`，处理原因后在原窗口点击「重新尝试」。

终端、连接配置输入框和 MCP 配置文本的右键菜单只有「复制」和「粘贴」。双击终端文字可以选词。Windows 中 Ctrl+Insert 可复制终端选区，接管后 Shift+Insert 可粘贴。在 Mac 上，右键点词或链接文字即可复制，右键点已有选区会保留选中的文本。接管后输入或粘贴会清除终端选区。密码框禁用复制。终端只在当前标签由你接管时允许粘贴，MCP 配置文本只读。

展开右侧的「Agent 执行记录」可以拖选时间、状态和命令，跨行或跨条复制。Mac 按 ⌘C，Windows 按 Ctrl+C 或 Ctrl+Insert。选区跨过折叠的长命令时，复制内容包含该命令的完整文本。

连接配置的输入框支持撤销、重做和按词选择。在 macOS 上按 ⌘Z 撤销、⇧⌘Z 重做，在 Windows 上按 Ctrl+Z 撤销、Ctrl+Y 重做。macOS 也可以从 Edit 菜单执行这些操作。连续输入可以一次撤销，粘贴和剪切各自单独撤销。

点击主机标签查看该会话。右侧三角展开最近 20 条 Agent 执行记录。断开后用标签上的 ↻ 重连，× 关闭选定会话。接管后可输入命令，正在运行的前台命令会收到 Ctrl-C。退出交互程序后再点「交还 Agent」。重连期间可继续在其他标签输入、接管或交还，也可以同时重连其他标签。重连会创建新的 shell，命令不会自动重放。顶部「皮肤」可切换五套界面和终端配色。用 Tab 和 Shift+Tab 可以在工具栏按钮间移动，用 Enter 或空格激活。Esc 关闭对话框并回到打开它的按钮。终端取得焦点后，Tab 继续用于远端补全。把鼠标停在主机标签可以查看完整的账号和地址。

终端程序用 OSC 8 标出的 HTTP、HTTPS 链接默认显示虚线下划线，鼠标悬停时变为实线。点击后会显示目标地址，确认后交给系统浏览器打开。

点击终端工具栏的 ⛶ 可以全屏查看终端，按 Esc 恢复原来的窗口。全屏时若要向远端发送 Esc，可以点击终端底部的 Esc 按钮。

接管后，全屏终端程序可以请求焦点通知、按格或按像素的鼠标坐标，并接收点击、拖动和滚轮事件。

查看长输出时，可以用滚轮、Shift + Page Up / Page Down 或终端右侧滚动条翻页。拖到最底部可继续跟随新输出。拖选文字时，把指针移到终端上沿或下沿，可以跨页选择。接管后，在实时终端按住 Alt（Mac 上为 Option）单击，可通过方向键移动远端光标。按住同一键拖动可以按列选取，选中的内容可用工具栏「复制」复制。

Windows 的「×」和「—」都会最小化窗口并保留 SSH 连接。点击标题栏下方工具栏最右侧的「退出」会关闭应用、后台和当前全部 SSH 会话。macOS 点关闭按钮会最小化。在系统菜单「垒桥终端」中选择「退出垒桥终端与后台」会断开此版本的全部 SSH。在 Mac 上也可以用 ⌘Q 退出。关闭网页或退出 MCP 客户端会保留后台。浏览器访问地址由 MCP 的 ssh_start_web 或程序参数 --ensure 返回，后台只在本机回环地址监听。

桌面与网页会话都可由 MCP 调用。通用 MCP 配置、pi agent 设置和可直接复制的提示词见 [MCP 配置与 Agent 提示词](MCP配置与Agent提示词.md)。

从 0.2.34 起，新建标签的 `session_id` 由主机地址、端口和 SSH 账号决定。关闭标签再连接同一目标，原网页书签和 MCP 主机编号都能继续使用，新 shell 的终端记录从头开始。升级前已有标签会沿用原编号，直到你主动关闭该标签。

## 本机文件

- macOS 使用 `~/Library/Application Support/BastionBridgeGPUI/`。
- Windows 使用 `%LOCALAPPDATA%\BastionBridgeGPUI\`。
- `profiles.json` 保存配置元数据，`credentials.json` 保存选择记住的明文密码，`known_hosts` 保存首次接受的主机密钥。Unix 配置文件权限为 0600。早期 GPUI 格式的密码会在首次读取配置时迁移。使用 `BASTION_GPUI_HOME` 可指定独立目录。
- `sessions.json` 只保存标签和目标，后台重启后显示为断开。分享时请分发发布包。

## 源码构建

桌面界面使用原生 GPUI。SSH、存储、HTTP 服务和 stdio MCP 使用 Rust。浏览器入口内嵌原版页面与 xterm.js，运行桌面程序不需要 Node、Python 或 WebView。

macOS 需要 Rust 和 Xcode Command Line Tools，执行 `cargo build --release --locked` 后运行 `sh scripts/package-macos.sh`。打包脚本另需 Python 3 生成依赖声明。Windows 需要 Rust MSVC、Visual Studio C++ Build Tools 和 Windows SDK（含 fxc.exe），在源码目录的 PowerShell 中执行 `.\scripts\package-windows.ps1`。使用 release 构建以将 Windows 图形着色器编入程序。测试命令为 `cargo test --locked`。

## 许可

原创代码采用 [MIT](LICENSE)。字体、终端组件和第三方源码的许可见 [第三方声明](THIRD-PARTY.md)。
