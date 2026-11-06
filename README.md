# CA Switch

CA Switch 是一个本地协议网关：把 **Anthropic / OpenAI Chat / OpenAI Responses / Gemini** 四条协议入口
统一转发到同一个当前激活的供应商，并允许在供应商之间一键切换。

- 只提供协议入口，不写任何客户端配置文件
- 供应商协议在表单「上游格式」中手动选择（落库为 `meta.apiFormat`）；只有官方登录类供应商等缺失该字段时才按 `api_format` → base URL 后缀 → 兜底推断
- 入口协议与上游协议不一致时走转换矩阵；没有转换器的组合返回 `501` 与可读说明
- 支持故障转移，但只在**同协议**供应商之间切换

## 与上游 CC Switch 的差异

代理内核（四条协议入口、转换矩阵、供应商切换、用量统计）保留并补齐，其余按「只做协议网关」大幅精简：

- **客户端收敛为单一入口**：原项目的 Claude Code / Codex / Gemini CLI 等多客户端支持与 13 个 app 的配置模块全部移除，`AppId` 收敛为 `claude`，界面不再出现任何客户端品牌。
- **不再接管客户端配置**：删除 `live/` 直写、`mode/` 与各 `*_config.rs` 写盘路径；新增、编辑、删除、切换只改数据库与全局激活指针，不写任何客户端文件。
- **移除 MCP / Skills 管理**：相关槽位、命令、表结构与前端入口一并删除，5 张下线表随 schema v28 迁移 DROP。
- **移除账号体系**：官方 OAuth 登录、托管账号、订阅额度、Copilot / Codex OAuth / xAI / Kimi 等授权与额度查询全部下线。
- **移除预设与模板**：供应商表单只保留自定义录入，预设供应商、OAuth 区块、客户端 UA 预设均删除，UA 改为自由输入。
- **移除会话日志用量链路**：只保留代理请求的 Token / 费用 / 请求数统计，会话日志采集、`session_log_sync` / `session_usage_dedup` 表与同步任务在 schema v27 下线。
- **界面精简**：导航只保留「供应商」，设置页下线「应用配置」，终端入口、配置目录、接管 / 路由目标等入口移除。

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

读法是「入口协议（行）→ 上游协议（列）」：先看请求从哪条入口进来，再看当前激活供应商配置的上游格式，交叉得到处理方式。上游协议在供应商的「上游格式」中手动选择，客户端不需要任何配置。

这张表**不对称**：Anthropic 入口可以转换到全部三种上游，而 Gemini 入口没有向外的转换器，只能对接 Gemini 上游；反方向（例如 Gemini 客户端配 Anthropic 供应商）会返回 `501`。

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
