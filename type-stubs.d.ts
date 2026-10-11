// Ambient stubs for Pi's runtime modules, which are provided by the agent host and are not
// dependencies of this extension. They are deliberately permissive: the type check exists to
// catch type errors in OUR code (remote-policy.ts, scrub.ts and the predicates in index.ts),
// not to re-check Pi's own API surface. Pi supplies the real types at runtime.
declare module "@earendil-works/pi-coding-agent" {
  export type AgentToolResult<T = unknown> = { content: { type: string; text?: string }[]; details?: unknown };
  export type ExtensionAPI = any;
  export type ToolDefinition = any;
  export function defineTool<T = unknown>(definition: any): any;
}

declare module "typebox" {
  export const Type: any;
}