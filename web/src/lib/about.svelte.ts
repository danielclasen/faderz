// Whether the About / What's new dialog is open: opened from the version
// lines of the connection details and the account menu, which close
// around it, so the dialog lives at the app's root (App.svelte).

export const about = $state<{ open: boolean }>({ open: false });

export function openAbout(): void {
  about.open = true;
}
