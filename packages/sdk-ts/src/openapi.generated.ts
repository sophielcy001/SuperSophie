// 从 docs/openapi.v1.yaml 生成。使用 npm run generate:contracts 重新生成。
export interface paths {
    "/api/v1/workspaces/{workspace_id}/integration": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        /** 读取当前集成令牌的动作权限 */
        get: operations["getIntegration"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/catalog": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        /** 按类型和稳定 ID 枚举可读资源 */
        get: operations["listCatalog"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/changes": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        /** 按授权拉取变更和可见归档墓碑 */
        get: operations["pollChanges"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/proposals": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** 提交带页面修订的候选修改供本地审阅 */
        post: operations["submitProposal"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/proposals/{proposal_id}": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                proposal_id: string;
            };
            cookie?: never;
        };
        /** 查询本集成对仍可读页面提交的建议状态 */
        get: operations["getProposal"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/search": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** 在授权范围内按关键词、类型和数据库属性检索 */
        post: operations["search"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/collections/{collection_id}": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                collection_id: components["parameters"]["Collection"];
            };
            cookie?: never;
        };
        /** 读取可访问数据库的属性定义 */
        get: operations["getCollection"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/query": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                collection_id: components["parameters"]["Collection"];
            };
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** 使用已授权属性筛选与排序记录 */
        post: operations["queryCollection"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/pages": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** 在授权子树或数据库中创建页面或记录 */
        post: operations["createPage"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/pages/{page_id}": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        /** 读取页面元数据及写入 ETag */
        get: operations["getPage"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /** 修改标题、图标或记录的普通属性 */
        patch: operations["patchPage"];
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/pages/{page_id}/content": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        /** 按相同授权表示分页读取正文块或 Markdown */
        get: operations["getPageContent"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/pages/{page_id}/backlinks": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        /** 列出可读页面中指向该页或其可读正文块的位置 */
        get: operations["getPageBacklinks"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/pages/{page_id}/references/{reference_id}": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
                /** @description 引用页中的引用块 ID */
                reference_id: string;
            };
            cookie?: never;
        };
        /** 读取一处授权块引用的当前源块 */
        get: operations["getBlockReference"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/pages/{page_id}/computed": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        /** 读取页面当前授权下的 Formula 和 Rollup 结果 */
        get: operations["getPageComputed"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/blocks/{block_id}": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                block_id: string;
            };
            cookie?: never;
        };
        /** 读取授权页中可见的正文块 */
        get: operations["getBlock"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/pages/{page_id}/block-operations": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** 在单页内原子追加、插入、更新、删除或移动正文块 */
        post: operations["applyBlockOperations"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/pages/{page_id}/archive": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** 将整个可读页面分支移入回收站 */
        post: operations["archivePage"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/assets": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** 上传原件并关联至目标页面 */
        post: operations["uploadAsset"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/workspaces/{workspace_id}/assets/{asset_id}/content": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                asset_id: string;
            };
            cookie?: never;
        };
        /** 下载授权页面所属的附件原件 */
        get: operations["downloadAsset"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        /**
         * Format: uri
         * @description 由知页桌面应用接收的稳定页面、正文块或 PDF 原件页码定位链接。
         */
        DesktopLocation: string;
        IntegrationPermissions: {
            scopes: ("content:read" | "content:create" | "content:update" | "content:archive" | "assets:read" | "assets:write" | "changes:read" | "proposals:create")[];
        };
        /** @description 服务生成的不透明游标。客户端原样保存和续传。 */
        FeedCursor: string;
        CatalogPage: {
            items: components["schemas"]["CatalogItem"][];
            /** @description 最后一页为 null。 */
            catalog_cursor: string | null;
            /** @description 首次请求返回的变更窗口起点。继续编目时为 null。 */
            change_cursor: string | null;
        };
        CatalogItem: {
            /** @constant */
            kind: "page";
            /** Format: uuid */
            id: string;
            revision: number;
            title: string;
            /** Format: uuid */
            parent_id: string | null;
        } | {
            /** @constant */
            kind: "collection";
            /** Format: uuid */
            id: string;
            revision: number;
            title: string;
            /** Format: uuid */
            container_page_id: string;
            schema_revision: number;
        } | {
            /** @constant */
            kind: "asset";
            /** Format: uuid */
            id: string;
            revision: number;
            name: string;
            /** Format: uuid */
            page_id: string;
            bytes: number;
            media_type: string;
        };
        ChangePage: {
            items: components["schemas"]["ChangeEvent"][];
            /** @description 在同一轮分页内固定的不透明水位标识。 */
            watermark: string;
            next_cursor: components["schemas"]["FeedCursor"];
            has_more: boolean;
        };
        ChangeEvent: {
            event_id: string;
            /** @enum {string} */
            kind: "page" | "collection" | "asset";
            /** Format: uuid */
            id: string;
            /** @description 对象原始修订。同一页面的派生失效可以共享原始修订，用 event_id 去重。 */
            revision: number;
            /** @enum {string} */
            action: "upsert" | "delete" | "derived.invalidated";
        } & unknown;
        ProposalDraft: {
            /** Format: uuid */
            page_id: string;
            base_revision: number;
            changes: {
                title?: string;
                icon?: string;
                properties?: components["schemas"]["WritableProperties"];
                operations?: components["schemas"]["BlockOperation"][];
            };
        };
        ProposalReceipt: {
            /** Format: uuid */
            id: string;
            /** Format: uuid */
            page_id: string;
            base_revision: number;
            /** @enum {string} */
            status: "pending" | "conflict" | "accepted" | "rejected" | "expired";
            /** @description 自 Unix 纪元起的毫秒数 */
            created_at: number;
            /** @description 自 Unix 纪元起的毫秒数 */
            expires_at: number;
            accepted_revision: number | null;
        };
        Page: {
            /** Format: uuid */
            id: string;
            /** Format: uuid */
            parent_id: string | null;
            page_path: string;
            page_link?: components["schemas"]["DesktopLocation"];
            schema_revision: number | null;
            title: string;
            icon: string;
            page_revision: number;
            /** @description 自 Unix 纪元起的毫秒数 */
            updated_at: number;
            properties: Record<string, never> | null;
            properties_restricted: boolean;
            restricted_property_ids?: components["schemas"]["RestrictedPropertyIds"];
        };
        /** @description 版本化的正文块。折叠、提示、公式、目录和附件块的 payload 形状见 BlockDraft。受限附件使用 restricted 占位。 */
        Block: {
            /** Format: uuid */
            id: string;
            type: string;
            schema_version: number;
            payload: Record<string, never>;
            children: components["schemas"]["Block"][];
            /** @default false */
            restricted?: boolean;
        };
        Backlinks: {
            items: {
                /** Format: uuid */
                page_id: string;
                /** Format: uuid */
                block_id: string;
                title: string;
                icon: string;
                /** @enum {string} */
                target_kind: "page" | "block";
            }[];
            next_cursor: string | null;
        };
        ReferenceSource: {
            /** Format: uuid */
            page_id: string;
            title: string;
            block: components["schemas"]["Block"];
            representation_version: string;
            /** @description 读取时的 Unix 毫秒数 */
            computed_at: number;
            /**
             * @description 当前源块在同一读取事务内投影
             * @constant
             */
            stale: false;
        };
        Operation: {
            /** Format: uuid */
            operation_id: string;
            /** Format: uuid */
            page_id: string;
            revision: number;
            created_block_ids: string[];
            source: components["schemas"]["IntegrationSource"];
        };
        Filter: {
            /** @description 属性 ID 或特殊值 title */
            field_id: string;
            /** @enum {string} */
            operator: "equal" | "contains" | "empty" | "notEmpty";
            value?: string | number | boolean | null;
        };
        SearchRequest: {
            query?: string;
            /** @enum {string} */
            kind?: "page" | "collection" | "record";
            /** Format: uuid */
            collection_id?: string;
            filters?: components["schemas"]["Filter"][];
            /** @default 50 */
            limit?: number;
            cursor?: string;
        };
        CollectionQuery: {
            filters?: components["schemas"]["Filter"][];
            sort?: {
                field_id: string;
                /** @enum {string} */
                direction: "asc" | "desc";
            };
            /** @default 50 */
            limit?: number;
            cursor?: string;
        };
        QueryPage: {
            items: unknown[];
            /** @description 不透明的当前查询快照标识 */
            snapshot_revision: string;
            next_cursor: string | null;
        };
        SearchHit: {
            /** Format: uuid */
            page_id: string;
            title: string;
            /** @enum {string} */
            kind: "page" | "collection" | "record";
            page_revision: number;
            /** Format: uuid */
            collection_id: string | null;
            /** Format: uuid */
            block_id: string | null;
            /** @enum {string} */
            source: "title" | "block" | "metadata" | "pdf";
            snippet: {
                before: string;
                matched: string;
                after: string;
            } | null;
            page_path: string;
            block_path: string | null;
            page_link?: components["schemas"]["DesktopLocation"];
            block_link?: components["schemas"]["DesktopLocation"] | null;
            /** @description 当 source 为 pdf 时提供。需要 assets:read 及附件所在页面的读取权限。 */
            pdf?: {
                /** Format: uuid */
                attachment_id: string;
                sha256: string;
                page_number: number;
                name: string;
                link: components["schemas"]["DesktopLocation"];
                content_path: string;
            };
        };
        Collection: {
            /** Format: uuid */
            id: string;
            /** Format: uuid */
            container_page_id: string;
            schema_revision: number;
            properties: {
                /** Format: uuid */
                id: string;
                name: string;
                /**
                 * @description date 是 YYYY-MM-DD 全天日期。dateTime 是带明确偏移且精确到秒的 RFC 3339 时间。
                 * @enum {string}
                 */
                type: "text" | "number" | "checkbox" | "date" | "dateTime" | "status" | "select" | "multiSelect" | "url" | "attachment" | "relation" | "rollup" | "formula" | "createdAt" | "updatedAt";
                /** @description 目标数据库可读时返回。 */
                relation?: {
                    /** Format: uuid */
                    collectionId: string;
                    /** Format: uuid */
                    inversePropertyId: string | null;
                };
                calculation?: components["schemas"]["Calculation"];
                options: {
                    /** Format: uuid */
                    id: string;
                    name: string;
                }[];
            }[];
        };
        /** @description 属性的计算规则。目标数据库不可读时省略 Rollup 规则。 */
        Calculation: {
            /** @constant */
            type: "formula";
            /** @description prop 的参数为稳定的属性 ID，标题使用 title。 */
            expression: string;
        } | {
            /** @constant */
            type: "rollup";
            /** Format: uuid */
            relationPropertyId: string;
            /** @enum {unknown} */
            function: "count" | "sum" | "min" | "max" | "average";
            /** Format: uuid */
            targetPropertyId: string | null;
        };
        ComputedPage: {
            /** Format: uuid */
            page_id: string;
            page_revision: number;
            /** @description 由原始页修订、读取主体可见的计算输入和规则及待计算状态得出的不透明版本。 */
            representation_version: string;
            /** @description 上次有效结果的计算时间，使用 Unix 毫秒。 */
            computed_at: number;
            /** @description 为 true 时可见输入已发生变化且重新计算仍在等待。 */
            stale: boolean;
            /** @description 按属性 ID 列出当前授权结果。待计算时为空对象。 */
            properties: {
                [key: string]: components["schemas"]["ComputedValue"];
            };
        };
        ComputedValue: {
            /** @constant */
            status: "value";
            value: string | number | boolean | null;
        } | {
            /** @constant */
            status: "error";
            /** @enum {unknown} */
            code: "CYCLE" | "LIMIT_EXCEEDED" | "RESTRICTED" | "TYPE_ERROR" | "DIVISION_BY_ZERO" | "OUT_OF_RANGE" | "INVALID_DATE";
        };
        Record: {
            /** Format: uuid */
            page_id: string;
            title: string;
            page_revision: number;
            created_at: number;
            updated_at: number;
            properties: Record<string, never>;
            properties_restricted: boolean;
            restricted_property_ids?: components["schemas"]["RestrictedPropertyIds"];
        };
        /** @description 含未显示成员的关联属性 ID。 */
        RestrictedPropertyIds: string[];
        /** @description 以稳定属性 ID 为键。普通属性可用 null 清除，关联属性使用逐项增删，未提交的属性保持原值。 */
        WritableProperties: {
            [key: string]: (string | number | boolean | null) | string[] | components["schemas"]["RelationDelta"];
        };
        RelationDelta: {
            add?: string[];
            remove?: string[];
        };
        BlockContent: {
            /** @constant */
            format: "blocks";
            /** Format: uuid */
            page_id: string;
            page_revision: number;
            blocks: components["schemas"]["Block"][];
            next_cursor: string | null;
        };
        MarkdownContent: {
            /** @constant */
            format: "markdown";
            /** Format: uuid */
            page_id: string;
            page_revision: number;
            markdown: string;
            losses: ("restricted_content" | "markdown_approximation")[];
            next_cursor: string | null;
        };
        BlockDraft: {
            /** @enum {string} */
            type: "paragraph" | "heading" | "codeBlock" | "horizontalRule" | "blockquote" | "bulletList" | "orderedList" | "listItem" | "taskList" | "taskItem" | "table" | "tableRow" | "tableCell" | "tableHeader" | "attachmentImage" | "attachmentFile" | "toggleBlock" | "callout" | "mathBlock" | "tableOfContents" | "blockReference";
            /** @description 段落可用 text 提交纯文本。块引用提交同一工作空间的 pageId 和 blockId。 toggleBlock 提交 title 和布尔值 collapsed。callout 提交 tone（note、tip 或 warning）。 以上两种容器至少含一个子块。mathBlock 提交 latex，最多 4,096 UTF-8 字节。 tableOfContents 使用空对象。attachmentFile 使用同页原件的 attachmentId UUID，需 assets:read。 */
            payload: Record<string, never>;
            children?: components["schemas"]["BlockDraft"][];
        };
        BlockOperation: {
            /** @constant */
            op: "append";
            /** Format: uuid */
            parent_block_id?: string | null;
            block: components["schemas"]["BlockDraft"];
        } | {
            /** @constant */
            op: "insert";
            /** Format: uuid */
            parent_block_id?: string | null;
            index: number;
            block: components["schemas"]["BlockDraft"];
        } | {
            /** @constant */
            op: "update";
            /** Format: uuid */
            block_id: string;
            block: components["schemas"]["BlockDraft"];
        } | {
            /** @constant */
            op: "delete";
            /** Format: uuid */
            block_id: string;
        } | {
            /** @constant */
            op: "move";
            /** Format: uuid */
            block_id: string;
            /** Format: uuid */
            parent_block_id?: string | null;
            /** @description 先取出被移动块后，在目标同级块中的零基位置 */
            index: number;
        };
        UploadReceipt: {
            /** Format: uuid */
            operation_id: string;
            /** Format: uuid */
            asset_id: string;
            /** Format: uuid */
            page_id: string;
            revision: number;
            source: components["schemas"]["IntegrationSource"];
            name: string;
            bytes: number;
            sha256: string;
            media_type: string;
        };
        IntegrationSource: {
            /** @constant */
            type: "integration";
            /** Format: uuid */
            id: string;
        };
        Error: {
            code: string;
            message: string;
            /** Format: uuid */
            request_id: string;
            retryable: boolean;
            details: null | Record<string, never>;
        };
    };
    responses: {
        /** @description 提交结果或同一幂等请求的原始结果 */
        Operation: {
            headers: {
                [name: string]: unknown;
            };
            content: {
                "application/json": components["schemas"]["Operation"];
            };
        };
        /** @description 请求未成功，应用错误代码对应本文档旁的接入指南。 */
        Error: {
            headers: {
                /** @description HTTP 401 返回 Bearer 认证提示。 */
                "WWW-Authenticate"?: "Bearer";
                /** @description HTTP 429 返回此值，单位为秒。 */
                "Retry-After"?: number;
                [name: string]: unknown;
            };
            content: {
                "application/json": components["schemas"]["Error"];
            };
        };
    };
    parameters: {
        FeedLimit: number;
        Workspace: string;
        Page: string;
        Collection: string;
        IdempotencyKey: string;
        /** @description 完整复制同一集成读取目标页时收到的 ETag，包括引号。 */
        IfMatch: string;
        /** @description 可同时提交读取 ETag 时的页面修订。服务端在首次写入时检查两者，重放时沿用原值。 */
        BaseRevision: number;
    };
    requestBodies: never;
    headers: {
        /** @description 与页面修订、集成和当前授权下的内容关联。 */
        ETag: string;
    };
    pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
    getIntegration: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 当前令牌的动作权限 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["IntegrationPermissions"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    listCatalog: {
        parameters: {
            query?: {
                limit?: components["parameters"]["FeedLimit"];
                /** @description 原样传入上一页的 catalog_cursor。首次请求省略。 */
                cursor?: components["schemas"]["FeedCursor"];
            };
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 资源编目及首次请求捕获的初始变更游标 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CatalogPage"];
                };
            };
            409: components["responses"]["Error"];
            410: components["responses"]["Error"];
            default: components["responses"]["Error"];
        };
    };
    pollChanges: {
        parameters: {
            query: {
                limit?: components["parameters"]["FeedLimit"];
                /** @description 首次编目的 change_cursor 或上一页的 next_cursor。 */
                cursor: components["schemas"]["FeedCursor"];
            };
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 同轮查询的水位、可见事件、续页标记和游标 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ChangePage"];
                };
            };
            409: components["responses"]["Error"];
            410: components["responses"]["Error"];
            default: components["responses"]["Error"];
        };
    };
    submitProposal: {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": components["parameters"]["IdempotencyKey"];
            };
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ProposalDraft"];
            };
        };
        responses: {
            /** @description 已提交候选修改的收据 */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ProposalReceipt"];
                };
            };
            412: components["responses"]["Error"];
            default: components["responses"]["Error"];
        };
    };
    getProposal: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                proposal_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 建议状态及接受后写入的修订 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ProposalReceipt"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    search: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SearchRequest"];
            };
        };
        responses: {
            /** @description 带匹配来源与定位路径的分页结果 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["QueryPage"] & {
                        items?: components["schemas"]["SearchHit"][];
                        /** @description 搜索含关键词且具备 assets:read 时，返回授予的页面范围内的 PDF 索引状态。 */
                        pdf_index?: {
                            pending: number;
                            indexed: number;
                            partial: number;
                            no_text: number;
                            failed: number;
                        };
                    };
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getCollection: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                collection_id: components["parameters"]["Collection"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 数据库与稳定属性标识 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Collection"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    queryCollection: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                collection_id: components["parameters"]["Collection"];
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CollectionQuery"];
            };
        };
        responses: {
            /** @description 仅含当前授权的记录和属性 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["QueryPage"] & {
                        /** Format: uuid */
                        collection_id: string;
                        schema_revision: number;
                        items?: components["schemas"]["Record"][];
                    };
                };
            };
            default: components["responses"]["Error"];
        };
    };
    createPage: {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": components["parameters"]["IdempotencyKey"];
            };
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    /** Format: uuid */
                    parent_id?: string;
                    /** Format: uuid */
                    collection_id?: string;
                    title: string;
                    /** @default file */
                    icon?: string;
                    properties?: components["schemas"]["WritableProperties"];
                } & (unknown | unknown);
            };
        };
        responses: {
            201: components["responses"]["Operation"];
            default: components["responses"]["Error"];
        };
    };
    getPage: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 当前授权内的页面元数据 */
            200: {
                headers: {
                    ETag: components["headers"]["ETag"];
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Page"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    patchPage: {
        parameters: {
            query?: never;
            header: {
                /** @description 完整复制同一集成读取目标页时收到的 ETag，包括引号。 */
                "If-Match": components["parameters"]["IfMatch"];
                "Idempotency-Key": components["parameters"]["IdempotencyKey"];
                /** @description 可同时提交读取 ETag 时的页面修订。服务端在首次写入时检查两者，重放时沿用原值。 */
                "X-Zhiye-Base-Revision"?: components["parameters"]["BaseRevision"];
            };
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    title?: string;
                    icon?: string;
                    properties?: components["schemas"]["WritableProperties"];
                } | unknown | unknown | unknown;
            };
        };
        responses: {
            200: components["responses"]["Operation"];
            default: components["responses"]["Error"];
        };
    };
    getPageContent: {
        parameters: {
            query?: {
                format?: "blocks" | "markdown";
                limit?: number;
                /** @description 上一响应的 next_cursor。响应为 null 时扫描结束。 */
                cursor?: string;
            };
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 当前页的块 */
            200: {
                headers: {
                    ETag: components["headers"]["ETag"];
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["BlockContent"] | components["schemas"]["MarkdownContent"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getPageBacklinks: {
        parameters: {
            query?: {
                limit?: number;
                /** @description 上次响应的 next_cursor。每次按当前授权读取。 */
                cursor?: string;
            };
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 按引用页和正文块标识分页的反向链接 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Backlinks"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getBlockReference: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
                /** @description 引用页中的引用块 ID */
                reference_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 在读取事务中计算的单层源块表示 */
            200: {
                headers: {
                    "Cache-Control"?: "no-store";
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ReferenceSource"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getPageComputed: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 计算发生在同一授权读事务，结果使用独立的表示版本。 */
            200: {
                headers: {
                    "Cache-Control"?: "no-store";
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ComputedPage"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getBlock: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                block_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 所属页面及块 */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        /** Format: uuid */
                        page_id: string;
                        page_revision: number;
                        page_path: string;
                        block_path: string;
                        page_link?: components["schemas"]["DesktopLocation"];
                        block_link?: components["schemas"]["DesktopLocation"];
                        block: components["schemas"]["Block"];
                    };
                };
            };
            default: components["responses"]["Error"];
        };
    };
    applyBlockOperations: {
        parameters: {
            query?: never;
            header: {
                /** @description 完整复制同一集成读取目标页时收到的 ETag，包括引号。 */
                "If-Match": components["parameters"]["IfMatch"];
                "Idempotency-Key": components["parameters"]["IdempotencyKey"];
                /** @description 可同时提交读取 ETag 时的页面修订。服务端在首次写入时检查两者，重放时沿用原值。 */
                "X-Zhiye-Base-Revision"?: components["parameters"]["BaseRevision"];
            };
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    operations: components["schemas"]["BlockOperation"][];
                };
            };
        };
        responses: {
            200: components["responses"]["Operation"];
            default: components["responses"]["Error"];
        };
    };
    archivePage: {
        parameters: {
            query?: never;
            header: {
                /** @description 完整复制同一集成读取目标页时收到的 ETag，包括引号。 */
                "If-Match": components["parameters"]["IfMatch"];
                "Idempotency-Key": components["parameters"]["IdempotencyKey"];
                /** @description 可同时提交读取 ETag 时的页面修订。服务端在首次写入时检查两者，重放时沿用原值。 */
                "X-Zhiye-Base-Revision"?: components["parameters"]["BaseRevision"];
            };
            path: {
                workspace_id: components["parameters"]["Workspace"];
                page_id: components["parameters"]["Page"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: components["responses"]["Operation"];
            default: components["responses"]["Error"];
        };
    };
    uploadAsset: {
        parameters: {
            query?: never;
            header: {
                /** @description 完整复制同一集成读取目标页时收到的 ETag，包括引号。 */
                "If-Match": components["parameters"]["IfMatch"];
                "Idempotency-Key": components["parameters"]["IdempotencyKey"];
                "X-Zhiye-Page-Id": string;
                /** @description 百分号编码的 UTF-8 文件名。 */
                "X-Zhiye-File-Name": string;
            };
            path: {
                workspace_id: components["parameters"]["Workspace"];
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/octet-stream": string;
            };
        };
        responses: {
            /** @description 附件及关联页面新版本的收据 */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["UploadReceipt"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    downloadAsset: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspace_id: components["parameters"]["Workspace"];
                asset_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description 已校验的附件原件。传输期间授权失效时连接会提前结束。 */
            200: {
                headers: {
                    "Content-Length": number;
                    "Content-Disposition": "attachment";
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            default: components["responses"]["Error"];
        };
    };
}
