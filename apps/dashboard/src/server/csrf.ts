export interface CsrfInput {
  readonly cookie?: string;
  readonly header?: string;
  readonly fetchSite?: string;
}

export function verifyCsrf(input: CsrfInput): boolean {
  if (!input.cookie || !input.header || input.fetchSite === "cross-site") return false;
  const length = Math.max(input.cookie.length, input.header.length);
  let difference = input.cookie.length ^ input.header.length;
  for (let index = 0; index < length; index += 1) {
    difference |= (input.cookie.charCodeAt(index) || 0) ^ (input.header.charCodeAt(index) || 0);
  }
  return difference === 0;
}
