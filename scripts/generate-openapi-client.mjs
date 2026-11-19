import { readFile, writeFile } from 'node:fs/promises';

const contract = JSON.parse(
  await readFile(new URL('../openapi/argus.json', import.meta.url), 'utf8'),
);
const operations = [];
for (const [path, methods] of Object.entries(contract.paths)) {
  for (const [method, operation] of Object.entries(methods)) {
    if (!operation.operationId) continue;
    operations.push(
      `  ${JSON.stringify(operation.operationId)}: { method: ${JSON.stringify(method.toUpperCase())}, path: ${JSON.stringify(path)} },`,
    );
  }
}
const output = `// Generated from openapi/argus.json. Do not edit.\nexport const API_OPERATIONS = {\n${operations.join('\n')}\n} as const;\n`;
await writeFile(
  new URL('../web/src/services/openapi.generated.ts', import.meta.url),
  output,
);
