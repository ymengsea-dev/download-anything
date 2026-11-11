// esbuild bundles imported CSS into dist/main.css and inlines PNGs as data: URLs.
declare module "*.css";
declare module "*.png" {
  const src: string;
  export default src;
}
