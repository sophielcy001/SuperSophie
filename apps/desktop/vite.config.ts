import { readFileSync, readdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const pdfRoot = dirname(createRequire(import.meta.url).resolve('pdfjs-dist/package.json'));
const pdfData = ['cmaps', 'standard_fonts'];

export default defineConfig({
  plugins: [react(), {
    name: 'local-pdf-data',
    configureServer(server) {
      server.middlewares.use('/pdf-data', (request, response, next) => {
        const match = request.url?.match(/^\/(cmaps|standard_fonts)\/([\w.-]+\.(?:bcmap|pfb|ttf))$/);
        if (!match) { next(); return; }
        try {
          response.setHeader('Content-Type', 'application/octet-stream');
          response.end(readFileSync(join(pdfRoot, match[1], match[2])));
        } catch { response.statusCode = 404; response.end(); }
      });
    },
    generateBundle() {
      for (const directory of pdfData) {
        for (const filename of readdirSync(join(pdfRoot, directory))) {
          this.emitFile({ type: 'asset', fileName: `pdf-data/${directory}/${filename}`,
            source: readFileSync(join(pdfRoot, directory, filename)) });
        }
      }
    },
  }],
  clearScreen: false,
  build: {
    target: 'es2022',
    rolldownOptions: {
      output: {
        codeSplitting: { groups: [
          { name: 'rich-text', test: /node_modules\/(?:@tiptap\/|prosemirror-)/ },
        ] },
      },
    },
  },
});

