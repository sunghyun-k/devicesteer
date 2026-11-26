// Lint for the viewer: TypeScript's recommended rules and React's hook rules, including the React
// Compiler's (eslint-plugin-react-hooks `recommended`), which flag code the compiler can't
// memoize safely.
import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import { defineConfig } from "eslint/config";
import tseslint from "typescript-eslint";

export default defineConfig(
  { ignores: ["dist", "src-tauri"] },
  js.configs.recommended,
  tseslint.configs.recommended,
  reactHooks.configs.flat.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: { globals: globals.browser },
    rules: {
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],
    },
  },
  {
    files: ["*.config.{ts,js}"],
    languageOptions: { globals: globals.node },
  },
);
