import { inspectEnvironment, inspectTool, type CreateSessionOptions } from "@aexhq/brain";
import { applicationTool } from "@aexhq/env-http";

export function composeApplication(options: CreateSessionOptions, baseUrl: string): CreateSessionOptions {
  const url = new URL("/environments/application", baseUrl).href;
  return { ...options, tools: options.tools?.map(placed => {
    const tool = inspectTool(placed);
    const environment = inspectEnvironment(tool.environment);
    if (environment.driver.driver !== "http" || environment.driver.url !== url) return placed;
    return applicationTool(placed, { env: tool.environment });
  }) };
}
