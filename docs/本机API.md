# 本机 API v1

在知页打开「切换工作空间 → 集成与 API」。为当前工作空间生成令牌，选择单页、页面子树或整个数据库。子树包含当前和日后新增的子页，数据库包含当前和日后新增的记录。初始权限为读取内容。创建、直接修改、归档、下载附件、上传附件、拉取增量变化和提交修改建议分别授权。上传还需要直接修改权限。

令牌生成时显示一次。将它保存到客户端的受保护配置，并从「授权资源与 ID」取得目标页面 ID。启用本机 API 后，窗口显示工作空间请求前缀。系统在每次启用时为 `127.0.0.1` 分配端口。知页退出或手动关闭 API 后，原端口停止服务。重新启动知页需要重新启用 API。

新建集成时可选择 1 小时、1 天、7 天或 30 天的令牌有效期，也可选择手动撤销。集成列表显示到期时间和过期状态。到期后的请求返回 `401 UNAUTHORIZED`，正在传输的原件在下次分块鉴权时停止发送。待审建议在接受时重新核对授权，已到期的授权会使建议进入冲突。需要继续使用时可为相应资源创建新的授权。

客户端通过 `Authorization: Bearer <token>` 发送令牌。服务检查 Host，拒绝带 Origin 的请求。同一台电脑的命令行、脚本和原生程序可以使用连接地址。[Bearer 请求头](https://www.rfc-editor.org/rfc/rfc6750#section-2.1)

## 版本与兼容

稳定契约从 `1.0.0` 开始，工作空间请求前缀为 `/api/v1/workspaces/{workspace_id}`。桌面、SDK、SQLite 格式、内容包和块 Schema 分别维护自己的版本。`page_revision` 标记对象的提交修订，`ETag` 和 `representation_version` 分别用于写入条件和派生表示，客户端按不透明值保存。[OpenAPI 3.1](https://spec.openapis.org/oas/v3.1.0.html) [当前契约](openapi.v1.yaml) [发布基线](contracts/rest-v1.0.0.yaml) [数据格式](数据格式.md)

v1 保留已发布的路径、方法、授权要求、写入前置条件、字段语义和已定义的响应。服务可增加可选响应字段，客户端应忽略自己尚未识别的字段。破坏兼容的变更使用新的路径主版本。停用旧版前发布迁移说明与弃用窗口。

`npm run check:contracts` 核对当前 OpenAPI 与固定基线中的路由、参数、授权、配额及已声明的 Schema，并核对 SDK 类型和 MCP 输入定义是否和规范同步。修改服务时还需运行回环验收，核对实际返回值和跨版本行为。基线记录首发契约，兼容扩展写入当前规范。

## 编目与增量缓存

1. 请求 `GET /catalog?limit=50`，保存首响应的 `change_cursor`。响应同时返回第一页资源元数据。页面、数据库和附件按 `(kind, id)` 排序。附件只在授权 `assets:read` 时列出。
2. 用 `GET /catalog?cursor=...` 原样传入上一页的 `catalog_cursor`，直到返回 `null`。编目游标从启动扫描起有效 15 分钟。普通提交可在扫描期间继续发生。
3. 枚举完成后，从第一步的 `change_cursor` 请求 `GET /changes?cursor=...`。在同一轮分页内，`watermark` 固定。若 `has_more` 为真，立即用 `next_cursor` 读取同轮下一页。若为假，将 `next_cursor` 持久化以继续后续轮询，即使本次 `items` 为空。
4. 收到 `upsert` 后按 ID 获取当前内容，收到 `delete` 后移除对应缓存对象。按 `event_id` 去重，按 `(kind, id, revision)` 防止重试回退。归档产生墓碑，恢复产生新的 `upsert`。页面事件用页面修订，数据库与附件各有独立的编目修订。
5. 收到 `derived.invalidated` 时，按事件 ID 去重，并使该页的派生缓存失效。按需使用同一授权重新读取引用预览、关联字段和计算属性。多个派生事件可以具有同一个 `revision`，该字段表示页的当前原始修订。用各派生响应的 `representation_version` 更新对应缓存，关联字段缓存使用页面响应的 ETag。

首个变更游标和第一页编目来自同一个 SQLite 读事务。随后枚举按当前授权读取。变化会回补已经扫描过的位置，扫描和回放完成后缓存可追上系统当前状态。新变化进入下一轮轮询。服务按当前权限过滤事件，水位及游标保持不透明。`changes:read` 为独立权限。分页默认 50 项，最大 100 项。

变更游标可以跨应用重启续读。重新启用 API 后，用当前监听地址和保存的游标继续请求。事件窗口至少 31 天，结束一轮查询会续发变更游标。授权发生变化时返回 `409 CURSOR_INVALID`。游标过期或落在已清理的事件之前时返回 `410 CURSOR_EXPIRED`。遇到这两个错误，从新编目重建并删除旧缓存中缺席的资源。接入方负责完成缓存清理。已经交付给客户端的数据仍由客户端持有。

派生事件覆盖块引用、关联的可见性和计算属性的依赖变化。工作空间比较提交前后的可读表示，并持久化变化。后台按批次发布，提交后可以重试。同一源操作最多为每个受影响的页面发布一项，当前授权和范围版本继续约束交付。事件只含受影响页的 ID 和通用变更元数据。后台可在源页的普通事件之后交付派生事件。[OpenAPI](openapi.v1.yaml)

## 搜索与数据库

将界面显示的请求前缀填入 `ZHIYE_BASE`，从客户端受保护配置加载 `ZHIYE_TOKEN`。响应的 `page_path` 和 `block_path` 是本机 API 相对路径，可配合当前监听地址及令牌精确取页或取块。`page_link` 和 `block_link` 可直接在关联协议的本机打开对应目标。[桌面定位链接](定位链接.md)

```sh
curl --fail-with-body --request POST \
  --header "Authorization: Bearer $ZHIYE_TOKEN" \
  --header "Content-Type: application/json" \
  --data '{"query":"文献整理","limit":20}' \
  "$ZHIYE_BASE/search"
```

搜索支持关键词、资源类型和数据库属性条件。设置 `kind` 可选择普通页、数据库或记录，对应 `page`、`collection`、`record`。在指定数据库时，可以省略关键词并通过属性筛选记录。结果含匹配来源、稳定 ID、当前修订与到对应块的路径。摘要从令牌可见的正文生成。

### PDF 检索

搜索有关键词时，授予 `content:read` 和 `assets:read` 的集成还可读取授权页面中的 PDF 文字层。命中的 `source` 为 `pdf`。`pdf` 对象包含附件 ID、文件名、SHA-256、从 1 起算的 `page_number`、桌面 `link` 和原件 `content_path`。同一文件的多个命中页可以随结果分页取得。

`pdf_index` 返回授权范围内的 `pending`、`indexed`、`partial`、`no_text` 和 `failed` 文件计数。已发布的可读 PDF 文本变化后，旧搜索游标可能返回 `409 CURSOR_INVALID`，可从第一页重新查询。只对隐藏附件执行索引提取而不修改原件或授权时，已签发给该集成的搜索游标仍可使用。[PDF 检索指南](PDF检索.md)

用 `GET /collections/{collection_id}` 读取当前 Schema。从 `properties` 取得属性 ID，再调用 `POST /collections/{collection_id}/query`。多个 `filters` 条件全部满足才返回。筛选的 `operator` 使用 `equal`、`contains`、`empty` 或 `notEmpty`。例如下列结构按数字属性匹配并按标题排序。把示例值换成 Schema 返回的属性 UUID。

```json
{
  "filters": [{"field_id":"<评分属性 UUID>","operator":"equal","value":3}],
  "sort": {"field_id":"title","direction":"asc"},
  "limit": 50
}
```

查询默认每页 50 条，最多 100 条。客户端将 `next_cursor` 原样放进下一次请求的 `cursor`，并保持令牌、条件和页大小。游标对应同一内容快照，15 分钟内有效。内容、Schema 或授权范围变化时，从第一页重新查询。后台索引继续构建时，待索引页面直接参与同一查询。

`date` 使用 `YYYY-MM-DD` 全天日期。`dateTime` 使用带明确偏移、精确到秒的 RFC 3339 字符串。普通属性读写保留输入的偏移，日期时间的等值筛选与排序按绝对时间比较。[桌面日期视图](数据库视图.md#日历和时间线)

## 读取与写入

读取页面元数据时保存响应头的完整 `ETag`，包括双引号。正文用 `GET /pages/{page_id}/content?format=blocks` 或 `format=markdown`。正文只在第一页附 Markdown 页标题，后续用 `next_cursor` 继续读取相同格式。Markdown 响应通过 `losses` 标记受限内容或格式降级，允许的页面引用和图片指向相应 API 路径。

页面属性修改、块操作和归档使用 `If-Match`，可同时带 `X-Zhiye-Base-Revision` 表示读取 ETag 时的页面修订。服务在首个写入事务校验提供的条件。同一请求的重试保留原修订、ETag、正文和幂等键。[HTTP If-Match](https://www.rfc-editor.org/rfc/rfc9110.html#name-if-match)

以下示例在页面末尾追加纯文本段落。把最近一次读取的 ETag 放入 `ZHIYE_ETAG`，把新生成的操作标识放入 `ZHIYE_REQUEST_KEY`。

```sh
curl --fail-with-body --request POST \
  --header "Authorization: Bearer $ZHIYE_TOKEN" \
  --header "If-Match: $ZHIYE_ETAG" \
  --header "Idempotency-Key: $ZHIYE_REQUEST_KEY" \
  --header "Content-Type: application/json" \
  --data '{"operations":[{"op":"append","parent_block_id":null,"block":{"type":"paragraph","payload":{"text":"文献整理笔记"}}}]}' \
  "$ZHIYE_BASE/pages/$ZHIYE_PAGE/block-operations"
```

块操作支持追加、插入、更新、删除和同页移动。一批最多 100 项，按提交顺序检查父子关系并一起保存。插入和移动的 `index` 从 0 开始。移动先从原位置取出目标，再按目标同级列表的索引插入。更新保留该块 ID，省略 `children` 时保留原子块。涉及受限块的覆盖、删除或移动会使批次失败。创建记录使用 `collection_id`，可同时提交 `properties`。修改记录只提交要改变的普通属性，以 `null` 清除已有值。

所有外部修改的 JSON 请求上限为 1 MiB。重试同一写入时保留正文、原 `ETag` 和原幂等键。7 天内，相同且仍有权限的请求会得到原收据。归档后的页面进入回收站，外部接口访问该页面会返回 404。保存来源可在页面「历史」查看。

### 正文内容块

`format=blocks` 返回折叠 `toggleBlock`、提示 `callout`、公式 `mathBlock`、目录 `tableOfContents` 和附件 `attachmentFile`。原生包使用 `schemaVersion`，本机 API 响应使用 `schema_version`。块操作和修改建议支持这五类 v1 内容。[序列化与 Markdown 示例](内容块.md)

`attachmentFile` 的 `payload.attachmentId` 指向本页原件，需要 `assets:read`。无权读取时，响应以 `restricted` 块占位。包含受限子块的容器保留可读正文并标记 `restricted: true`。对该容器执行更新、删除或移动会返回 `403 PARTIAL_CONTENT`。其中可读的独立子块仍可编辑。

`format=markdown` 对复杂块返回 `markdown_approximation`。其中折叠输出完整可读正文，提示使用引用标记，公式保留 LaTeX，目录使用稳定占位。可读附件提供 `/assets/{attachment_id}/content` 下载路径。

### 记录关联

用来源数据库的属性 ID 在 `properties` 中提交 `add` 和 `remove`。两者均可省略。指定的记录须可访问且属于属性的目标数据库。双向关系会同步反向字段并记录两端的修订来源。创建记录时写入双向关联需有创建权限和修改已有目标记录的权限。

```json
{
  "properties": {
    "cd91bdcf-c93d-44ab-8c76-ad510ed66391": {
      "add": ["7fd74c9b-cb0a-47e7-bcd8-e2eb59da56d0"],
      "remove": ["f1f01cba-c620-409e-8328-1ff0893d4e5d"]
    }
  }
}
```

读取页面或查询记录时，关联字段只包含可读目标的 ID 数组。含不可用目标的字段列在 `restricted_property_ids`。用数组或 `null` 整体替换这类字段会返回 `403 PARTIAL_CONTENT`。可读目标仍可逐项增删。查询数据库定义时，目标数据库可读的关联返回 `relation.collectionId` 和 `inversePropertyId`。筛选、排序和分组入口使用所支持的普通属性。

### 计算属性

`GET /pages/{page_id}/computed` 使用 `content:read` 和当前资源授权，返回以属性 ID 索引的 `properties`、`representation_version`、`computed_at` 和 `stale`。成功单元格的 `status` 为 `value`，其 `value` 可为空。出错单元格的 `status` 为 `error`，同时携带 `code`。可读普通页返回空的属性映射。首次读取在当前事务内生成结果。

已有结果的可见输入变化且后台正在重算时，`stale` 为 `true`，`properties` 为空对象，`computed_at` 保留上次可用结果的 Unix 毫秒时间。待计算期间可以保留客户端的加载状态，收到对应的 `derived.invalidated` 或短暂等待后重取。`stale` 为 `false` 时可接收这次的属性结果。隐藏输入的变化会保持受限读者的结果、版本和计算时间。

派生结果采用 `Cache-Control: no-store`。表示版本随页面原始修订、计算规则和实际可读输入改变。修改原始记录继续从 `GET /pages/{page_id}` 取得 ETag。该路径的 `properties` 只返回原始属性值。只改动依赖页的数值时，引用页的原始修订与写入 ETag 保持稳定。缓存可按 `derived.invalidated` 重取计算结果，源页自己的 `upsert` 也应刷新其结果。

汇总只累加有效范围中的关联记录。公式中因作用域而受限的属性返回 `RESTRICTED`。隐藏来源的改动不改变该主体的计算版本或产生其可读的派生事件。计算字段通过桌面属性管理调整，REST 普通属性写入与筛选继续使用相应支持的类型。[函数与错误代码](计算属性.md)

### 块引用与反向链接

正文中的 `collectionView` 是保存视图的描述。可读嵌入页和目标数据库、且视图仍存在时返回 `collectionId` 和 `viewId`。受限条目按通用占位投影。Markdown 以授权数据库链接表示，保留格式降级标记。

可读的 `blockReference` 在 `format=blocks` 中携带 `payload.pageId` 和 `payload.blockId`。Markdown 以块 API 路径表示这处引用并标记格式降级。受限源返回通用占位。覆盖、删除、移动受限引用或提交指向受限源的新引用会返回 `403 PARTIAL_CONTENT`。

`GET /pages/{page_id}/references/{reference_id}` 中的 `reference_id` 是引用页中的引用块 ID。接口返回当前授权下的源块和源页标题，包含 `representation_version`、`computed_at` 与 `stale`。计算发生在同一读取事务，响应使用 `Cache-Control: no-store`。如果源块已移除、归档或超出范围，返回 `404 RESOURCE_NOT_FOUND`。源块正文变化会更新派生版本，引用页的原始修订仍由引用页自身提交决定。

`GET /pages/{page_id}/backlinks` 列出可读的引用位置。分页参数为 `limit` 和 `cursor`，续页传入上一响应的 `next_cursor`。条目包含引用页与其中的块 ID，`target_kind` 表示所指的是整页或正文块。每次按当前授权读取。图片及附件源块的描述、派生内容和反向链接需有 `assets:read`。

### 上传附件

`POST /assets` 的请求正文为原件字节，类型为 `application/octet-stream`。用 `X-Zhiye-Page-Id` 提供目标页 ID，用 `X-Zhiye-File-Name` 提供经过 UTF-8 百分号编码的文件名，另带 `If-Match` 和 `Idempotency-Key`。原件校验后关联到目标页，成功响应带附件 ID、字节数、SHA-256 和提交修订。

上传默认上限 100 MiB，可在「集成与 API」为工作空间设置 1 至 100 MiB 的单文件上限。下载需要单独的 `assets:read` 权限。传输中断时，客户端应丢弃不完整文件。

## 提交与审阅修改建议

在可读页面的基础上授予 `proposals:create`。用 `GET /pages/{page_id}` 取得 `page_revision`，将它用作提交时的 `base_revision`。需要提供新的 `Idempotency-Key`。候选修改可以组合标题、图标、普通属性和块操作，字段规则与直接写入一致。

```json
{
  "page_id": "<页面 UUID>",
  "base_revision": 3,
  "changes": {
    "operations": [{"op":"append","block":{"type":"paragraph","payload":{"text":"待审阅的文献整理结果"}}}]
  }
}
```

向 `POST /proposals` 提交上面的 JSON 会保存候选内容，正式页沿用原修订。7 天内用同一请求键可重取原始收据。通过 `GET /proposals/{proposal_id}` 查询本集成提交的状态。接受后，`accepted_revision` 给出实际保存的修订。

用户可从「页面操作 → 修改建议」或「集成与 API → 审阅修改建议」查看原文、候选内容、属性差异、来源和提交时间。接受前会再次核对用户预览的版本与当前授权。审阅期间原文被改动时，建议进入 `conflict`，可重新预览后再次选择接受。未处理的建议在 30 天后过期。拒绝、过期或离开审阅窗口时，正式页保持原样。接受写成一条新修订，来源显示在页面历史中。

## 路径

| 方法 | 相对工作空间前缀的路径 | 用途 |
|---|---|---|
| GET | `/integration` | 读取当前令牌的动作权限 |
| GET | `/catalog` | 枚举授权元数据并取得增量起点 |
| GET | `/changes` | 按游标读取授权事件及归档墓碑 |
| POST | `/search` | 关键词、类型及数据库属性检索 |
| GET | `/collections/{collection_id}` | 当前可访问的数据库 Schema |
| POST | `/collections/{collection_id}/query` | 记录筛选、排序及分页 |
| GET | `/pages/{page_id}` | 页面元数据、写入 ETag 和定位路径 |
| GET | `/pages/{page_id}/content` | 块或 Markdown 正文分页 |
| GET | `/pages/{page_id}/computed` | 读取授权计算结果及表示版本 |
| GET | `/pages/{page_id}/backlinks` | 授权的反向链接位置分页 |
| GET | `/pages/{page_id}/references/{reference_id}` | 单处块引用的授权派生表示 |
| GET | `/blocks/{block_id}` | 精确读取授权页中的可见块 |
| POST | `/pages` | 在授权子树中创建页面或在授权数据库中建记录 |
| PATCH | `/pages/{page_id}` | 修改标题、图标和提交的普通属性 |
| POST | `/pages/{page_id}/block-operations` | 原子提交同一页内的块操作 |
| POST | `/pages/{page_id}/archive` | 移入回收站，调用者须有整段分支的权限 |
| POST | `/assets` | 上传原件并关联目标页 |
| GET | `/assets/{asset_id}/content` | 下载可访问页面的原件 |
| POST | `/proposals` | 将候选修改交给本地用户审阅 |
| GET | `/proposals/{proposal_id}` | 查询自己提交的建议状态 |

完整请求和响应字段见接口契约。通过客户端或 TypeScript 脚本接入时可使用已构建的适配器和 SDK。[OpenAPI](openapi.v1.yaml) [MCP 与 SDK](MCP与SDK.md)

## 错误与恢复

| HTTP 状态 | 处理 |
|---|---|
| 400 | 检查筛选字段、数据类型或请求格式 |
| 401 | 查看 `WWW-Authenticate: Bearer`，检查令牌、工作空间、到期时间和撤销状态 |
| 403 | 检查动作权限与目标中受限的块、字段或子页 |
| 404 | 检查目标稳定 ID 和当前资源范围 |
| 409 | 幂等键已用于另一项请求，或游标无效。按错误代码重新生成请求标识或重建查询 |
| 410 | 编目或变更游标过期，重新编目并清理旧缓存 |
| 412 | 重新读取 ETag，比较内容后提交 |
| 413 | 缩小正文、操作批次或附件 |
| 415 | JSON 请求使用 `application/json`，上传原件使用 `application/octet-stream` |
| 428 | 带上最近读取的目标页 ETag |
| 429 | 按 `Retry-After` 等待。每个集成在 10 秒窗口内最多 60 个请求 |

桌面编辑遇到较新的页面修订时会保留本机草稿。用「对比版本」分别选择要保留的标题和正文，再以读到的修订为条件保存。对比时若又有新写入，可返回草稿并重新比较。无未保存改动时，从「页面操作 → 重新读取页面」刷新外部改动。

集成将当前页所在分支移入回收站时，可直接从「回收站」恢复该分支。原编辑器继续保留本机草稿，恢复后的版本可在「对比版本」读取。[保存故障与恢复](故障恢复.md#编辑过程中出现保存失败)

## 从源码复验

在源码根目录安装锁定依赖后，构建 SDK、MCP 和回环夹具。Windows 将最后一条命令的可执行文件路径改为 `target/debug/examples/api_contract.exe`。

```sh
npm run build:integrations
cargo build --locked -p zhiye-desktop --example api_contract
npm run test:api-v1 -- target/debug/examples/api_contract api-v1-report.json
```

验收脚本启动桌面的实际监听逻辑和独立的合成 SQLite 数据根。报告记录 OpenAPI 中的 20 个操作、响应 Schema、作用域、游标及八个 MCP 工具。可在命令末尾传入另一版本的 SDK 模块，核对页面读取与全文搜索。[开发记录与证据](../开发记录.md#2026-11-16--k19-api-v1-契约冻结)
