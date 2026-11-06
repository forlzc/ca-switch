# CA Switch

CA Switch 是一个本地协议网关：把 **Anthropic / OpenAI Chat / OpenAI Responses / Gemini** 四条协议入口
统一转发到同一个当前激活的供应商，并允许在供应商之间一键切换。

- 只提供协议入口，不写任何客户端配置文件
- 供应商协议在表单「上游格式」中手动选择（落库为 `meta.apiFormat`）；只有官方登录类供应商等缺失该字段时才按 `api_format` → base URL 后缀 → 兜底推断
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

## 转换矩阵

行是入口协议（客户端连进来的路径），列是当前激活供应商的上游协议。上游协议在供应商的「上游格式」中手动选择，客户端不需要做任何配置。

| 入口 \ 上游 | Anthropic Messages | OpenAI Chat | OpenAI Responses | Gemini |
|---|---|---|---|---|
| Anthropic Messages | 透传 | 转换 | 转换 | 转换 |
| OpenAI Chat | 转换 | 透传 | 转换 | `501` |
| OpenAI Responses | 转换 | 转换 | 透传 | `501` |
| Gemini | `501` | `501` | `501` | 透传 |

- **透传**：入口与上游协议相同，请求与响应原样转发。
- **转换**：请求与响应（含 SSE 流式）双向改写，长推理期间每 15 秒心跳保活。
- **`501`**：该组合没有可用的转换器，返回 `501` 与可读说明，不做静默降级。

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
