import type { Config } from "tailwindcss";

// Tailwind supplies the CSS reset and the sr-only utility; colours, type and spacing live in styles/tokens.css.
export default {
  content: ["./src/**/*.{tsx,ts}"],
  theme: { extend: {} },
  plugins: [],
} satisfies Config;
