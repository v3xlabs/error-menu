import type { Linter } from "eslint";
import v3xlabs from "eslint-plugin-v3xlabs";

const config: Linter.Config[] = [
  { ignores: ["dist/**", "src/api/schema.gen.ts"] },
  ...v3xlabs.configs.recommended,
  ...v3xlabs.configs.solid,
  ...v3xlabs.configs.tailwindcss,
  // A flat config file is loaded by its default export; the format leaves no choice.
  { files: ["eslint.config.ts"], rules: { "import/no-default-export": "off" } },
];

export default config;
