# 接入本机 MCP 和 TypeScript SDK

在知页的「集成与 API」启用本机 API。为接入方建立专用集成，先选择授权资源。只读检索选择「读取内容」。希望由使用者审阅写入时，再选择「提交修改建议」。直接写入分别选择创建或修改权限。界面给出的工作空间请求前缀随监听端口变化，重启知页并重新启用 API 后更新该地址。

从源码安装锁定依赖并构建两个包。使用 Node.js 24。

```sh
npm ci
npm run build:integrations
```

把环境变量 `ZHIYE_BASE` 与 `ZHIYE_TOKEN` 写入当前用户可读的私有文件，或由启动 MCP 客户端的进程注入。下面是占位配置。

```dotenv
ZHIYE_BASE=http://127.0.0.1:46832/api/v1/workspaces/<工作空间UUID>
ZHIYE_TOKEN=<专用集成令牌>
```

## MCP 客户端

在支持 stdio 的客户端为知页配置一个命令。使用本机 Node 的完整路径，以及实际的私有环境文件和构建目录路径。

```json
{
  "mcpServers": {
    "zhiye": {
      "command": "/绝对路径/node",
      "args": [
        "--env-file=/绝对路径/zhiye.env",
        "/项目绝对路径/packages/mcp/dist/cli.js"
      ]
    }
  }
}
```

每次列举工具时会读取当前集成权限。调用时按当前令牌和目标资源再次检查。工具的 `structuredContent` 可供程序读取，文本结果附相同 JSON。调用失败时读取 `structuredContent.error.code`、`request_id` 和 `retry_after_seconds`。搜索、页面和块返回 API 路径与桌面定位链接。写入结果可返回新页面链接和已创建正文块的链接。[桌面定位链接](定位链接.md) [MCP 工具契约](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) [stdio 传输](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)

| 工具 | 主要参数与结果 |
|---|---|
| `search_knowledge` | `query`、可选资源类型／数据库属性、分页条件。返回命中页、块和修订。 |
| `get_page` | `page_id`、可选 `format`。格式 `blocks`、`markdown` 或 `metadata` 返回 `page`、`etag` 及所请求的正文。格式 `computed` 返回 `computed`，包含授权汇总、公式结果和表示版本。块与 Markdown 支持游标。 |
| `get_block` | `block_id`。返回原块、所属页及当前修订。 |
| `query_database` | `collection_id`、可选 `query`。返回属性定义、记录和游标。 |
| `create_page` | `page` 含标题以及父页或数据库 ID，另带 `idempotency_key`。 |
| `append_blocks` | `page_id`、`base_revision`、`if_match`、`idempotency_key` 和追加操作数组。 |
| `update_page` | 与追加共用页面写入前提。使用 `patch` 修改标题、图标、普通属性和关联，或通过 `operations` 提交单页块操作。 |
| `propose_page_changes` | `proposal` 含 `page_id`、`base_revision` 和 `changes`，另带 `idempotency_key`。桌面审阅后才写入正式页。 |

追加和更新的 `base_revision` 与 `if_match` 取自同一次 `get_page`。遇到网络中断且提交结果未知，重试使用原输入及原幂等键。遇到 `REVISION_CONFLICT` 时重新读取页面，核对后再生成新的请求。

读取示例依次调用 `search_knowledge({"query":"文献整理"})`、`get_page` 和 `get_block`。从命中项取得页面及块 ID。若只有页面标题命中，可从 `get_page` 返回的正文选块。授权了建议时，将该页的 `page_revision` 和候选操作提交给 `propose_page_changes`，再在桌面「页面操作 → 修改建议」或「集成与 API → 审阅修改建议」查看。

## TypeScript SDK

工作区中的代码可从 `@zhiye/sdk-ts` 导入。REST 输入输出类型从同一份 OpenAPI 生成。SDK 提供编目、变更、搜索、页面与块、数据库、资源写入、附件和建议。返回的 `ZhiyeApiError.problem` 含 HTTP 状态、应用错误码、请求 ID 和限流等待时间。调用者根据错误码与幂等键决定是否重试。附件下载可通过返回值的 `body` 流式读取。[本机 API](本机API.md) [OpenAPI](openapi.v1.yaml)

从对象响应可读取 `page_link` 或 `block_link`。已知页面和块 ID 时，也可用 `client.pageLink(pageId, blockId)` 构造同一地址。

用 `client.getPageBacklinks(pageId, { cursor, limit })` 分页读取反向链接。用 `client.getBlockReference(pageId, referenceId)` 获取当前可读的源块，其中 `referenceId` 是引用页正文中 `blockReference` 自己的 ID。MCP 的 `get_block` 读取正文中已有的引用描述。

使用 `client.patchPage` 或 MCP 的 `update_page` 更新关系时，在 `properties` 中用关联属性 ID 传入 `RelationDelta`，其 `add` 与 `remove` 接受目标记录 ID。响应提供当前授权可读的关系成员。缓存收到 `derived.invalidated` 后，可用 `client.getPage` 重新获取最新关联。[关联写入示例](本机API.md#记录关联)

使用 `client.getPageComputed(pageId)` 或 MCP 的 `get_page` 传 `format: "computed"` 读取授权汇总和公式。`stale: true` 表示正在等待重算，`properties` 此时为空。收到派生失效事件后可重取，缓存以 `stale: false` 的结果及其 `representation_version` 更新。[计算属性接口](本机API.md#计算属性)

```ts
import { ZhiyeClient } from '@zhiye/sdk-ts';

const client = new ZhiyeClient({
  baseUrl: process.env.ZHIYE_BASE ?? '',
  token: process.env.ZHIYE_TOKEN ?? '',
});
const pageId = process.env.ZHIYE_PAGE ?? '';
const requestKey = process.env.ZHIYE_REQUEST_KEY ?? '';
const { data: page, etag } = await client.getPage(pageId);
await client.patchPage(page.id, { title: '复核后的资料标题' }, {
  etag,
  baseRevision: page.page_revision,
  idempotencyKey: requestKey,
});
```

两个可运行命令从同一环境文件加载地址与令牌。第二条中的页面、修订、键和候选文字由调用者提供。

```sh
node --env-file=/绝对路径/zhiye.env examples/integration.mjs search '文献整理'
node --env-file=/绝对路径/zhiye.env examples/integration.mjs propose "$ZHIYE_PAGE" "$ZHIYE_REVISION" "$ZHIYE_REQUEST_KEY" '待审阅的段落'
```

资源索引示例维护 `kind / id / revision` 和变更游标。初次按编目枚举再回放变更，下一次传入上次返回的状态。收到 `CURSOR_INVALID` 或 `CURSOR_EXPIRED` 时重建索引。返回的 `rebuilt` 为真时，清除旧范围的派生缓存。日常续读根据 `invalidatedDerivedPages` 重新读取受影响页面的引用预览，按 `representation_version` 更新。处理完缓存更新后持久化新的游标。原始数据和原件继续根据普通变更事件更新。[资源索引示例](../examples/resource-index.mjs)

## 已验收版本

| 入口 | 版本与协议 | 运行环境 |
|---|---|---|
| MCP 服务 | MCP TypeScript SDK 1.32.2，stdio 协议 `2025-11-25` | Node.js 24 |
| 回环 HTTP 与独立 MCP 客户端 | 知页 0.29.0，MCP TypeScript SDK 1.32.2 客户端协商 `2025-11-25`。20 个 REST 操作、八个工具及四种成功提交后断线重放通过 | macOS ARM64，Node.js 24 |
| 真实回环与独立 MCP 客户端 | 知页 0.26.0，独立 MCP TypeScript SDK 1.32.2 客户端协商 `2025-11-25`，通过八个工具及同连接撤权复验 | macOS ARM64，Node.js 24 |
| REST v1 客户端兼容 | SDK 0.25.0 对知页 0.26.0 的 v1 服务完成页面读取与全文搜索 | macOS ARM64，Node.js 24 |
| 独立 MCP 客户端 | MCP Inspector CLI 0.21.2。知页 0.13.0 验收八个工具，0.14.0 复验正文块定位 | macOS ARM64，Node.js 24 |
| 独立 REST 调用 | SDK 0.13.0 验收写入与回放，0.14.0 复验搜索、页面和块定位字段，0.17.0 验收受限关联的局部增删，0.18.0 验收按授权计算和依赖失效 | macOS ARM64，Node.js 24 |
| 独立 stdio 计算读取 | MCP TypeScript SDK 1.32.2 客户端在知页 0.18.0 验收 `get_page` 的 `computed` 格式 | macOS ARM64，Node.js 24 |

工具枚举、调用与结构化结果遵循协商的 MCP 协议。复验命令和环境见 API 指南与开发记录。Windows 的实机验收仍待完成。[MCP 工具协议](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) [API 复验](本机API.md#从源码复验) [开发记录](../开发记录.md)
