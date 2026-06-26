// Flat ESLint config scoped to the e2e workspace only. It lives beside e2e's
// own package.json and is never referenced by ui/, so `ui/`'s lint and CI
// `npm ci` do not pull these dev-deps in.
import js from '@eslint/js';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  {
    ignores: [
      'node_modules/',
      'test-results/',
      'playwright-report/',
      'playwright/.cache/',
      'eslint.config.js',
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    languageOptions: {
      parserOptions: {
        project: './tsconfig.json',
      },
    },
    rules: {
      '@typescript-eslint/no-floating-promises': 'off',
      '@typescript-eslint/no-explicit-any': 'error',
    },
  },
);
