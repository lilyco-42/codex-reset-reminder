import { Toaster as Sonner, type ToasterProps } from "sonner";

// 注册表那份 sonner.tsx 依赖 next-themes 取主题；本项目没有 next-themes
// （窗口是固定深色），所以这里显式钉 dark，而不是为了一个 toast 再引一套主题库。
export function Toaster(props: ToasterProps) {
  return (
    <Sonner
      theme="dark"
      position="top-center"
      richColors
      closeButton
      toastOptions={{
        style: { background: "var(--card)", color: "var(--card-foreground)", border: "1px solid var(--border)" },
      }}
      {...props}
    />
  );
}
