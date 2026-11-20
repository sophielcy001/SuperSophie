// 从 docs/openapi.v1.yaml 生成。使用 npm run generate:contracts 重新生成。
export const restInputs = {
  "search": {
    "$ref": "#/$defs/SearchRequest"
  },
  "collectionQuery": {
    "$ref": "#/$defs/CollectionQuery"
  },
  "createPage": {
    "type": "object",
    "additionalProperties": false,
    "required": [
      "title"
    ],
    "oneOf": [
      {
        "required": [
          "parent_id"
        ],
        "not": {
          "anyOf": [
            {
              "required": [
                "collection_id"
              ]
            },
            {
              "required": [
                "properties"
              ]
            }
          ]
        }
      },
      {
        "required": [
          "collection_id"
        ],
        "not": {
          "required": [
            "parent_id"
          ]
        }
      }
    ],
    "properties": {
      "parent_id": {
        "type": "string",
        "format": "uuid"
      },
      "collection_id": {
        "type": "string",
        "format": "uuid"
      },
      "title": {
        "type": "string"
      },
      "icon": {
        "type": "string",
        "default": "file"
      },
      "properties": {
        "$ref": "#/$defs/WritableProperties"
      }
    }
  },
  "patchPage": {
    "type": "object",
    "additionalProperties": false,
    "anyOf": [
      {
        "required": [
          "title"
        ]
      },
      {
        "required": [
          "icon"
        ]
      },
      {
        "required": [
          "properties"
        ]
      }
    ],
    "properties": {
      "title": {
        "type": "string"
      },
      "icon": {
        "type": "string"
      },
      "properties": {
        "$ref": "#/$defs/WritableProperties"
      }
    }
  },
  "proposal": {
    "$ref": "#/$defs/ProposalDraft"
  },
  "operations": {
    "type": "array",
    "minItems": 1,
    "maxItems": 100,
    "items": {
      "$ref": "#/$defs/BlockOperation"
    }
  },
  "appendOnly": {
    "type": "array",
    "minItems": 1,
    "maxItems": 100,
    "items": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "op",
        "block"
      ],
      "properties": {
        "op": {
          "const": "append"
        },
        "parent_block_id": {
          "type": [
            "string",
            "null"
          ],
          "format": "uuid"
        },
        "block": {
          "$ref": "#/$defs/BlockDraft"
        }
      }
    }
  },
  "contentFormat": {
    "type": "string",
    "enum": [
      "blocks",
      "markdown"
    ],
    "default": "blocks"
  },
  "contentLimit": {
    "type": "integer",
    "minimum": 1,
    "maximum": 100,
    "default": 50
  },
  "contentCursor": {
    "type": "string"
  },
  "uuid": {
    "type": "string",
    "format": "uuid"
  },
  "idempotencyKey": {
    "type": "string",
    "minLength": 1,
    "maxLength": 128,
    "pattern": "^[!-~]+$"
  },
  "baseRevision": {
    "type": "integer",
    "minimum": 1,
    "maximum": 4294967295
  }
};
export const restDefinitions = {
  "SearchRequest": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "query": {
        "type": "string",
        "maxLength": 200
      },
      "kind": {
        "type": "string",
        "enum": [
          "page",
          "collection",
          "record"
        ]
      },
      "collection_id": {
        "type": "string",
        "format": "uuid"
      },
      "filters": {
        "type": "array",
        "maxItems": 48,
        "items": {
          "$ref": "#/$defs/Filter"
        }
      },
      "limit": {
        "type": "integer",
        "minimum": 1,
        "maximum": 100,
        "default": 50
      },
      "cursor": {
        "type": "string"
      }
    }
  },
  "Filter": {
    "type": "object",
    "additionalProperties": false,
    "required": [
      "field_id",
      "operator"
    ],
    "properties": {
      "field_id": {
        "type": "string",
        "description": "属性 ID 或特殊值 title"
      },
      "operator": {
        "type": "string",
        "enum": [
          "equal",
          "contains",
          "empty",
          "notEmpty"
        ]
      },
      "value": {
        "type": [
          "string",
          "number",
          "boolean",
          "null"
        ]
      }
    }
  },
  "CollectionQuery": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "filters": {
        "type": "array",
        "maxItems": 48,
        "items": {
          "$ref": "#/$defs/Filter"
        }
      },
      "sort": {
        "type": "object",
        "required": [
          "field_id",
          "direction"
        ],
        "additionalProperties": false,
        "properties": {
          "field_id": {
            "type": "string"
          },
          "direction": {
            "type": "string",
            "enum": [
              "asc",
              "desc"
            ]
          }
        }
      },
      "limit": {
        "type": "integer",
        "minimum": 1,
        "maximum": 100,
        "default": 50
      },
      "cursor": {
        "type": "string"
      }
    }
  },
  "WritableProperties": {
    "type": "object",
    "description": "以稳定属性 ID 为键。普通属性可用 null 清除，关联属性使用逐项增删，未提交的属性保持原值。",
    "additionalProperties": {
      "oneOf": [
        {
          "type": [
            "string",
            "number",
            "boolean",
            "null"
          ]
        },
        {
          "type": "array",
          "items": {
            "type": "string"
          }
        },
        {
          "$ref": "#/$defs/RelationDelta"
        }
      ]
    }
  },
  "RelationDelta": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "add": {
        "type": "array",
        "uniqueItems": true,
        "items": {
          "type": "string",
          "format": "uuid"
        }
      },
      "remove": {
        "type": "array",
        "uniqueItems": true,
        "items": {
          "type": "string",
          "format": "uuid"
        }
      }
    }
  },
  "ProposalDraft": {
    "type": "object",
    "additionalProperties": false,
    "required": [
      "page_id",
      "base_revision",
      "changes"
    ],
    "properties": {
      "page_id": {
        "type": "string",
        "format": "uuid"
      },
      "base_revision": {
        "type": "integer",
        "minimum": 1
      },
      "changes": {
        "type": "object",
        "additionalProperties": false,
        "minProperties": 1,
        "properties": {
          "title": {
            "type": "string"
          },
          "icon": {
            "type": "string"
          },
          "properties": {
            "$ref": "#/$defs/WritableProperties"
          },
          "operations": {
            "type": "array",
            "maxItems": 100,
            "items": {
              "$ref": "#/$defs/BlockOperation"
            }
          }
        }
      }
    }
  },
  "BlockOperation": {
    "oneOf": [
      {
        "type": "object",
        "additionalProperties": false,
        "required": [
          "op",
          "block"
        ],
        "properties": {
          "op": {
            "const": "append"
          },
          "parent_block_id": {
            "type": [
              "string",
              "null"
            ],
            "format": "uuid"
          },
          "block": {
            "$ref": "#/$defs/BlockDraft"
          }
        }
      },
      {
        "type": "object",
        "additionalProperties": false,
        "required": [
          "op",
          "index",
          "block"
        ],
        "properties": {
          "op": {
            "const": "insert"
          },
          "parent_block_id": {
            "type": [
              "string",
              "null"
            ],
            "format": "uuid"
          },
          "index": {
            "type": "integer",
            "minimum": 0
          },
          "block": {
            "$ref": "#/$defs/BlockDraft"
          }
        }
      },
      {
        "type": "object",
        "additionalProperties": false,
        "required": [
          "op",
          "block_id",
          "block"
        ],
        "properties": {
          "op": {
            "const": "update"
          },
          "block_id": {
            "type": "string",
            "format": "uuid"
          },
          "block": {
            "$ref": "#/$defs/BlockDraft"
          }
        }
      },
      {
        "type": "object",
        "additionalProperties": false,
        "required": [
          "op",
          "block_id"
        ],
        "properties": {
          "op": {
            "const": "delete"
          },
          "block_id": {
            "type": "string",
            "format": "uuid"
          }
        }
      },
      {
        "type": "object",
        "additionalProperties": false,
        "required": [
          "op",
          "block_id",
          "index"
        ],
        "properties": {
          "op": {
            "const": "move"
          },
          "block_id": {
            "type": "string",
            "format": "uuid"
          },
          "parent_block_id": {
            "type": [
              "string",
              "null"
            ],
            "format": "uuid"
          },
          "index": {
            "type": "integer",
            "minimum": 0,
            "description": "先取出被移动块后，在目标同级块中的零基位置"
          }
        }
      }
    ]
  },
  "BlockDraft": {
    "type": "object",
    "additionalProperties": false,
    "required": [
      "type",
      "payload"
    ],
    "properties": {
      "type": {
        "type": "string",
        "enum": [
          "paragraph",
          "heading",
          "codeBlock",
          "horizontalRule",
          "blockquote",
          "bulletList",
          "orderedList",
          "listItem",
          "taskList",
          "taskItem",
          "table",
          "tableRow",
          "tableCell",
          "tableHeader",
          "attachmentImage",
          "attachmentFile",
          "toggleBlock",
          "callout",
          "mathBlock",
          "tableOfContents",
          "blockReference"
        ]
      },
      "payload": {
        "type": "object",
        "description": "段落可用 text 提交纯文本。块引用提交同一工作空间的 pageId 和 blockId。 toggleBlock 提交 title 和布尔值 collapsed。callout 提交 tone（note、tip 或 warning）。 以上两种容器至少含一个子块。mathBlock 提交 latex，最多 4,096 UTF-8 字节。 tableOfContents 使用空对象。attachmentFile 使用同页原件的 attachmentId UUID，需 assets:read。"
      },
      "children": {
        "type": "array",
        "items": {
          "$ref": "#/$defs/BlockDraft"
        }
      }
    }
  }
};
