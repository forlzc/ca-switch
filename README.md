# CA Switch

CA Switch 是一个本地协议网关：把 **Anthropic / OpenAI Chat / OpenAI Responses / Gemini** 四条协议入口
统一转发到同一个当前激活的供应商，并允许在供应商之间一键切换。

- 只提供协议入口，不写任何客户端配置文件
- 供应商协议自动推断（`api_format` → base URL 后缀 → 家族兜底）
- 入口协议与上游协议不一致时走转换矩阵；没有转换器的组合返回 `501` 与可读说明
- 支持故障转移，但只在**同协议**供应商之间切换

## 入口地址

默认监听 `127.0.0.1:15721`：

| 入口协议 | Base URL |
|---|---|
| Anthropic Messages | `http://127.0.0.1:15721` |
| OpenAI Chat Completions | `http://127.0.0.1:15721/v1` |
| OpenAI Responses | `http://127.0.0.1:15721/v1` |
| Gemini | `http://127.0.0.1:15721/v1beta` |

端口以实际监听为准，可在设置中修改。

## 开发

```bash
pnpm install
pnpm tauri dev
```

数据目录：`~/.ca-switch/`（数据库 `ca-switch.db`）。

## 致谢

CA Switch 基于 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) 修改而来。
感谢原作者及所有贡献者的工作。

本分支由 [forlzc](https://github.com/forlzc) 维护，仓库地址：
[forlzc/ca-switch](https://github.com/forlzc/ca-switch)。
