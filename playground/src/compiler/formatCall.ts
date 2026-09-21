/** Keep the original argument text, including integers beyond JavaScript's precision. */
export function formatCall(signature: string, argumentsJson: string): string {
  const args = argumentsJson.trim();
  const values = args.startsWith("[") && args.endsWith("]") ? args.slice(1, -1) : args;
  return `${signature.split("(")[0]}(${values})`;
}
