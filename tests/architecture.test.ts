/** Resolve nested TypeScript imports to enforce product/service ownership. */
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import ts from 'typescript';
import { expect, it } from 'vitest';

it('keeps common and services independent and workflows behind contracts', (): void => {
  const root: string = path.resolve('src/app');
  const violations: string[] = [];
  function inspect(directory: string): void {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const file: string = path.join(directory, entry.name);
      if (entry.isDirectory()) { inspect(file); continue; }
      if (!file.endsWith('.ts')) continue;
      const relative: string = path.relative(root, file).split(path.sep).join('/');
      const source: ts.SourceFile = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
      function visit(node: ts.Node): void {
        let specifier: string | undefined;
        if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) specifier = node.moduleSpecifier.text;
        if (ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword && node.arguments[0] && ts.isStringLiteral(node.arguments[0])) specifier = node.arguments[0].text;
        if (ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument) && ts.isStringLiteral(node.argument.literal)) specifier = node.argument.literal.text;
        if (specifier) {
          const target: string = specifier.startsWith('.') ? path.relative(root, path.resolve(path.dirname(file), specifier)).split(path.sep).join('/') : specifier;
          if (relative.startsWith('common/') && /^(products|services)\//.test(target)) violations.push(`${relative} -> ${target}`);
          if (relative.startsWith('services/') && target.startsWith('products/')) violations.push(`${relative} -> ${target}`);
          if (relative.includes('/use-cases/') && (/^(electron|node:)/.test(target) || target.includes('/providers/') || target.endsWith('/repository/settings/file.ts') || target.endsWith('/router.ts'))) violations.push(`${relative} -> ${target}`);
        }
        ts.forEachChild(node, visit);
      }
      visit(source);
    }
  }
  inspect(root);
  expect(violations).toEqual([]);
});
