# caswitch 精简重构：进度与待办

> 更新时间：2026-11-05 16:49（GMT+8）
> 仓库：`/workspace/cc-switch`　远端：`https://github.com/forlzc/cc-switch.git`
> 分支：`caswitch` → 本轮改名 **`ca-switch`**
> 基线：tag `caswitch-base` / `origin/fork/build` (`bc4c4c45`)，本地领先 11 个提交
> 工作区：干净

---

## 1. 一句话现状

P0–P1 与 P2 的 **2a、2b-1、2e（含 2d）、2b-2** 已全部落地并逐步提交，每步都过了编译与单测。
**P2 只剩 2c 与 2f**，之后是 P3、P4、P5。当前后端 `cargo check --all-targets` 0 error，
`cargo test --lib` 3665 passed，仅剩 2 个基线偶发失败。

---

## 2. 分支与提交清单

| 提交 | 阶段 | 内容 |
|---|---|---|
| `cee21206` | 基线后 | provider-form 自定义 User-Agent 预设分组展示 |
| `5aa2dacd` | P1 | 移除 MCP 与 Skills 全套支持 |
| `6c0a82af` | 2a | 引入 `Protocol` 枚举与 `Matrix::plan` 转换矩阵（与 `AppType` 并存编译，行为零变化） |
| `f316c24f` | 2b-1 | `infer::of_provider` 四级协议推断证据链 |
| `9ad93d6a` | 2e-1 | 代理侧摘除 Stack 路由 |
| `af275fb0` | 2e-2 | 切换收敛为全局单激活指针（InUse / Direct 合并） |
| `66912c95` | 2e-3a | 代理侧与 `mode` 解耦，统一走 `ProviderService::active_provider(db, app)` |
| `760b7dd5` | 2e-3b | 新增供应商只入库 |
| `cb693617` | 2e-3c | 编辑 / 删除只操作库，激活项不可删 |
| `3d1096f2` | 2e-3 + 2d | 移除 `live/`、`mode/` 与客户端配置管理面（84 文件 / −30977 行） |
| `851f7c58` | 2b-2 | schema v25：协议化 + 全局单激活迁移 |

**安全网**

- tag `caswitch-base`：重构前基线。
- 分支 `caswitch-pre-v25` → `3d1096f2`：v25 数据库迁移前备份。
- `Database::init` 在 `version > 0 && version < SCHEMA_VERSION` 时自动生成 pre-migration 备份文件。

---

## 3. 已锁定的决策

| 编号 | 决策 | 落地形态 |
|---|---|---|
| A′ | 全局单激活，一个上游服务四个入口 | `providers.is_current` 上建唯一索引；DAO 统一走 `ProviderService::active_provider(db, app)` |
| B′ | 只提供协议，不再按客户端配置 / 托管客户端 | 删 `live/`、`mode/` 与 13 个 `*_config.rs` 写盘路径；增删改切换只改 DB 与激活指针 |
| C′ | 故障转移仅允许同协议 | **待 2c 在 forwarder 落地校验** |
| D′② | `proxy_config` = 代理行为参数表，按入口协议分四行 | 四行保留 `app_type` 主键，新增 `protocol` 列标记：`anthropic←claude` / `openai_responses←codex` / `openai_chat←grokbuild` / `gemini←gemini`，行为参数零变化 |
| — | Gemini 不新增转换 | 矩阵空白格返回 501 + 可读文案 |

---

## 4. 已完成里程碑要点

**P1 / 2a / 2b-1 / 2e 系列**：从外到内摘除 MCP / Skills；引入协议模型与矩阵（不改行为、随时可停）；
四级证据链 `meta.apiFormat` → codex TOML `wire_api` → base URL 后缀 → 家族兜底，纯 origin 不推断；
切换 / 新增 / 编辑 / 删除全部收敛到 DB 与全局激活指针，不再写任何客户端文件。

**2e-3 + 2d（`3d1096f2`）**：删除 `live/`、`mode/`、全部 `*_direct` / `*_editor`、`live.rs` 及 4 个孤儿文件；
`patch/`、`floor/`、`model_label/` 上提为顶层模块，`editor.rs` 收敛为纯 DB 载荷；
命令层去掉接管 / 模式 / Stack 命令、启动刷新、退出清理与代理 resync；补修 macOS 分支的 `sha256_hex` 引用。

**2b-2（`851f7c58`）**：`providers` PK 收敛为 `id`，新增 `protocol` / `protocol_source` / `auth_style` /
`base_url` / `api_key` / `default_model`；v24→v25 迁移逐行用 `infer::of_provider` 推断、id 去重
（同上游合并取最强证据 / 不同上游加 `-2` 后缀）、`providers_legacy` 留档 + `migration_report`、选全局激活项；
`proxy_config` 四行按 ② 打上入口协议标签。

> **三处有意的方案偏差**（详见 `caswitch-P2-迁移方案-v2.md` §10）：保留 `app_type` 兼容列、
> 沿用 `is_current` 列名、`proxy_config` 保留 `app_type` 主键而只新增 `protocol` 列。
> 原因：历史迁移 `migrate_v13_to_v14` 会把 `proxy_config` 整表重建回 app_type 结构，
> 若按原方案把 `protocol` 设为主键，v13 之前的旧库升级路径会断。228 处 DAO 调用点签名零改动。

---

## 5. 验证基线

| 项 | 结果 |
|---|---|
| `cargo check --all-targets` | **0 error** |
| `cargo test --lib` | **3665 passed** / 6 ignored / 2 failed |
| 2 个失败 | `proxy::hyper_client::tests::bytes_with_limit_*`，`caswitch-base` 基线同样失败，与本次改动无关 |
| 新增专项测试 | `database::schema::tests::migrate_v24_to_v25_collapses_namespaces_and_tags_protocols` **ok**（PK 收敛、去重改名、全局单激活、四协议映射、留档与报告） |
| 前端 `pnpm build` | **本轮未重跑**（后端改动为主，待 2f 统一验证） |
| 手工四入口连通 | **未做**（依赖 2c） |

---

## 6. 待办工作（按执行顺序）

### 6.1 2c — forwarder 接矩阵（下一步，改动最小、可独立验证）

现状实测：`Matrix::plan` 定义在 `src-tauri/src/protocol.rs:217`，但**目前只在 `protocol.rs` 的单元测试里被调用**，
`src-tauri/src/proxy/` 下没有任何生产代码引用它。

1. `proxy/handler_context.rs` / `proxy/provider_router.rs` 确定入口协议，映射关系：
   `claude→Anthropic`、`codex→OpenAiResponses`、`grokbuild→OpenAiChat`、`gemini→Gemini`
2. `proxy/forwarder.rs` 读激活供应商的 `protocol` 列，调 `Matrix::plan(inbound, upstream)`
3. `Passthrough` 直转；`Convert` 走已有转换器；`Unsupported` 返回 **501** + 中文可读文案
   （例："当前供应商为 Gemini 协议，OpenAI Chat 入口不支持，请改用 Claude Code 入口或切换供应商"）
4. 钉死 `Matrix::covers()` 12 格断言：**4 透传 + 5 转换 + 7 不支持**（`protocol.rs` 已有覆盖测试，确认口径一致）
5. 顺带落实 C′：故障转移换供应商时校验协议相同
6. **验收**：四条入口 × 对应上游各手工连通一次；7 个空白格全部 501 且文案可读

### 6.2 2f — 前端四入口 + 收尾清理

- 前端改造清单见 `caswitch-精简重构规划.md` §六；四条 base URL + 配置片段**只展示不写文件**，配复制按钮
- 清理大删除后残留的 **约 241 处 `dead_code` 警告**（`codex_config.rs` 占 79 处，全是 Stack / 官方模型目录 / CLI 候选符号）
- 删 5 张已下线的表：`mcp_servers`、`skills`、`skill_repos`、`prompts`、`proxy_live_backup`
  （**v25 迁移里特意推迟到此处**：备份 / 恢复与并发测试仍读写它们，现在删会破坏回归覆盖）
- `pnpm build` 全量验证
- **验收**：`git grep AppType` 归零；命令数 380 → ~200；四入口前端可配可切

### 6.3 P3 — 强制默认模型（2–2.5 天）

1. `default_model` 落库 + 前端「设为默认模型」（复用 `FetchedModelPicker`）
2. 出站统一改写：三协议改 body，Gemini 改 URI 路径
3. 5 处依赖模型目录的判定改为静态能力位
4. 单测：四协议 `model` 断言 + gemini URI 断言
5. **验收**：agent 请求任意模型，上游始终收到默认模型；用量页与实际上游一致

### 6.4 P4 — Chat→Responses 转换链路（4–6 天，不可压缩）

需求 4 落点是 `OpenAiChat → OpenAiResponses`，需成对新增转换器：

- 请求方向 `transform_chat_responses.rs`（`chat_request_to_responses`）
- 响应方向 `streaming_responses_chat.rs`（`responses_response_to_chat`）
- 难点：工具调用、流式 SSE 事件序列
- 可选补充：`chat → anthropic`（复用 `transform.rs` 的 `openai_to_anthropic` 请求转换，补响应与流式反向）
- round-trip 单测 + wiremock 集成；OpenAI SDK 脚本打 `/v1/chat/completions`、上游切 responses 手工验证
- **其余 6 个空白格永远不做**，只保留 501 + 文案（投入产出比不成立）
- **验收**：流式 / 非流式、工具调用、图片、长流全通

### 6.5 P5 — 改名与收尾（2–3 天）

| 项 | 位置 | 说明 |
|---|---|---|
| 包名 | `package.json` | `caswitch` |
| crate | `Cargo.toml` `name` / `[lib] name` | `caswitch` / `caswitch_lib`，同步 `main.rs` |
| 打包 | `tauri.conf.json` | `productName: caswitch`、`identifier: com.caswitch.desktop`（换 identifier 才能与原版并存） |
| 数据目录 | `~/.cc-switch/` → `~/.caswitch/` | **必须写迁移**：新目录不存在时从旧目录复制 db |
| i18n | 5 × 4,829 | 引用扫描裁剪，预计剩 ~2,500 |
| 文档资产 | `assets/partners` 19MB、`docs` 16MB、`README_{JA,DE}.md`、`CHANGELOG.md` | 删除或重写 |
| 首启向导 | 新增 | 四条 base URL + 配置片段（只展示不写文件）+ 复制按钮 |

> ⚠️ **命名一致性待确认**：规划文档里产品名 / 包名 / crate / 数据目录都写作 `caswitch`（无连字符），
> 而本轮分支按用户要求改名为 `ca-switch`（带连字符）。P5 落地前需要统一口径，
> 否则会出现「分支名 ≠ 产品名 ≠ 包名 ≠ identifier」的长期不一致。

---

## 7. 技术债与已知风险

| 项 | 说明 | 处置 |
|---|---|---|
| 约 241 处 `dead_code` 警告 | 大删除后残留，不阻塞编译 | 2f 清理 |
| 2 个基线偶发失败 | `hyper_client::bytes_with_limit_*` | 记录为已知，不修 |
| `app_type` / `is_current` 兼容列 | v25 有意保留，DAO 用 `?N = ?N` 恒真条件保持签名不变 | 2f 后统一收敛 |
| 5 张已下线表 | v25 未删 | 2f 删 |
| 前端构建未验证 | 本轮只跑后端 | 2f 补 |
| 命名不一致 | 分支 `ca-switch` vs 产品名 `caswitch` | P5 前确认 |
| 远程分支策略 | 本轮 push 新建 `origin/ca-switch`；`origin/fork/build` **未同步** | 若需 CI 基于新分支构建，另行确认 |

---

## 8. 常用命令

```bash
cd /workspace/cc-switch
git status
cd src-tauri && cargo check --all-targets && cargo test --lib
cd .. && pnpm build          # 2f 时
```

## 9. 相关文档

- `caswitch-精简重构规划.md` — 总规划、P0–P5 分解、协议矩阵、前端清单、数据迁移
- `caswitch-P0-基线.md`、`caswitch-P1-完成报告.md`
- `caswitch-P2-迁移方案.md`（v1）、`caswitch-P2-迁移方案-v2.md`（§9 状态表、§10 2b-2 落地备注）
