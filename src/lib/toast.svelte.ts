// A tiny global toast/status surface. The app previously had no place to show
// success/failure — persistence and connection errors only reached the
// console. Anything user-facing that succeeds or fails quietly should toast.

export type ToastKind = "info" | "success" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

export const toastStore = $state<{ list: Toast[] }>({ list: [] });

let nextId = 1;

export function dismiss(id: number) {
  toastStore.list = toastStore.list.filter((t) => t.id !== id);
}

function push(kind: ToastKind, message: string, ttlMs: number) {
  const id = nextId++;
  toastStore.list = [...toastStore.list, { id, kind, message }];
  if (ttlMs > 0) {
    setTimeout(() => dismiss(id), ttlMs);
  }
}

export const toast = {
  info: (message: string) => push("info", message, 4000),
  success: (message: string) => push("success", message, 4000),
  // Errors linger longer since they usually need action/awareness.
  error: (message: string) => push("error", message, 8000),
};
