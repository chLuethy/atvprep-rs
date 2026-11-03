/** @type {import('tailwindcss').Config} */
// Tailwind supplies the CSS reset and the sr-only utility; colours, type and spacing live in styles/tokens.css.
module.exports = {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: { extend: {} },
  plugins: [],
};
