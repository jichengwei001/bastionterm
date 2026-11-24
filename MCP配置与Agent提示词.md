# MCP 配置与 Agent 提示词

点击「MCP 设置」，选择「通用 MCP」或「pi agent」，复制当前程序的完整路径配置。把 baoleiji-pty 项合并到客户端已有的 mcpServers 中。稳定版与 GPUI 版并用时请使用两个不同的服务名称。下方以应用放在 /Applications 为例。

```json
{
  "mcpServers": {
    "baoleiji-pty": {
      "command": "/Applications/BastionTerm.app/Contents/MacOS/bastion-bridge-gpui",
      "args": ["--mcp"]
    }
  }
}
```

Windows 上请先解压并将 EXE 放在长期使用的位置，再从应用复制配置。

在 Pi 中使用时，选择「pi agent」复制配置，它已包含 `"exposure": "direct"`。将服务项合并到个人配置 `~/.pi/agent/mcp.json`（Windows 为 `%USERPROFILE%\.pi\agent\mcp.json`），或受信任项目的 `.pi/mcp.json`。运行 `pi mcp list` 检查连接和工具数，在已打开的 Pi 中输入 `/reload` 重新加载。

首次调用 SSH 工具时，程序会按需启动后台。多个 Agent 可以共享已有会话。若之前已完全退出应用，需重新连接目标主机才能运行远端命令。

可直接交给 Agent 的提示词：

> 使用 baoleiji-pty。先调用 ssh_list_profiles，根据我指定的目标选择 profile_id，再调用 ssh_connect。给我返回 web_url。所有操作显式传入返回的 session_id，不能按网页当前标签推断目标。owner 为 human 时等待我交还，busy 时先查询 active_id。命令会保留 cwd 和环境，请写清工作目录。ssh_execute 返回 running 后用同一任务 id 和 ssh_poll 继续查询，把上次 offset 传入读取增量。断线先核对结果，不能自动重放。只有我要求中断或断开时才调用 ssh_cancel 或 ssh_disconnect。

9 个工具为 ssh_start_web、ssh_list_profiles、ssh_connect、ssh_list_sessions、ssh_status、ssh_execute、ssh_poll、ssh_cancel、ssh_disconnect。单次命令上限 128000 个 UTF-8 字节，结果与终端缓冲各保留最近 200 万字符。poll 的 offset 以 Unicode 字符计数。

