// localStorage can throw (blocked site data, private windows), so every access is guarded.

export function stored(key: string) {
  try {
    return localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

/** Saves `value` under `key`, or removes the key when `value` is empty. */
export function store(key: string, value: string) {
  try {
    if (value) localStorage.setItem(key, value);
    else localStorage.removeItem(key);
  } catch {}
}
