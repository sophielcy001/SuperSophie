#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later

import { readFile } from 'node:fs/promises';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { ZhiyeApiError, ZhiyeClient } from '@zhiye/sdk-ts';
import { createZhiyeServer } from './server.js';

try {
  const client = new ZhiyeClient({
    baseUrl: process.env.ZHIYE_BASE ?? '',
    token: process.env.ZHIYE_TOKEN ?? '',
  });
  const manifest = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'));
  const server = createZhiyeServer(client, manifest.version);
  await server.connect(new StdioServerTransport());
} catch (error) {
  const message = error instanceof ZhiyeApiError ? error.problem.message : '无法启动知页 MCP 服务。';
  process.stderr.write(`知页 MCP：${message}\n`);
  process.exitCode = 1;
}
