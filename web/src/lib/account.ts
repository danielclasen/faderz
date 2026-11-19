// How the header names the signed-in user: by initials, never by an
// email address, which a principal can be in oidc and proxy mode.

/** Whether `user` reads as an email address. */
export function isEmail(user: string): boolean {
  return user.includes('@');
}

/**
 * Up to two initials of `user`: of its first and last word, the local
 * part of an email address split at dots, dashes, underscores and plus
 * signs as well; `?` where no word has a letter or digit.
 */
export function initials(user: string): string {
  const at = user.indexOf('@');
  const base = at >= 0 ? user.slice(0, at) : user;
  const words = base.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const picked = words.length >= 2 ? [words[0]!, words[words.length - 1]!] : words;
  const out = picked.map((w) => [...w][0]!).join('');
  return out === '' ? '?' : out.toLocaleUpperCase();
}
