// Text files imported with Vite's `?raw` suffix arrive as strings.
declare module "*.md?raw" {
  const content: string;
  export default content;
}
