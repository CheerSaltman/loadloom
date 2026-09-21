/// <reference types="vite/client" />

// 为什么需要这个文件
// ────────────────
// src/main.tsx 第 4 行有 `import "@/index.css";`。
// TypeScript 只认 .ts / .tsx / .d.ts / .json 这类已知扩展名，对 .css 必须存在
// 一份模块声明才能解析。
//
// 此前它能编译通过，靠的是 tsconfig.json 里的 `baseUrl: "."`：有了 baseUrl，
// paths 映射（"@/*": ["./src/*"]）会把 @/index.css 落到 ./src/index.css 并被容忍。
//
// TypeScript 7 移除了 baseUrl，随之一并移除了对未知扩展名的这条容忍路径，
// 于是 src/main.tsx:4 报错：
//   Cannot find module or type declarations for side-effect import of '@/index.css'.
//
// vite/client 自带 `declare module '*.css'` 等通配声明，是 Vite 官方推荐的引入方式。
// 显式引用它之后，@/index.css 由通配声明匹配，不再依赖任何 paths 解析行为，
// 对 TypeScript 5.x 与 7.x 同样成立。
