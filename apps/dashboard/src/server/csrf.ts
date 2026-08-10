export interface CsrfInput {
  readonly cookie?: string;
  readonly header?: string;
  readonly fetchSite?: string;
}

export function verifyCsrf(input: CsrfInput): boolean {
  if (!input.cookie || !input.header || input.fetchSite === "cross-site") return false;
  return equalsConstantTime(input.cookie, input.header);
}
export function equalsConstantTime(left: string, right: string): boolean {
  const length = Math.max(left.length, right.length);
  let difference = left.length ^ right.length;
  for (let index = 0; index < length; index += 1) {
    difference |= (left.charCodeAt(index) || 0) ^ (right.charCodeAt(index) || 0);
  }
  return difference === 0;
}
