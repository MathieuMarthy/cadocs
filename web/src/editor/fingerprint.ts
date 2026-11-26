// FNV-1a, 32 bits, in hex, as `cadocs_content::schema::fingerprint` computes it. Not SHA-256:
// `crypto.subtle` is missing over plain HTTP. No imports, so Node clients of the tests load it.

export function fingerprint(text: string) {
  let hash = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(text)) hash = Math.imul(hash ^ byte, 0x01000193);
  return (hash >>> 0).toString(16).padStart(8, "0");
}
