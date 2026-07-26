export interface Probes {
  readonly system: string;
  readonly instructions: string;
  readonly prompt: string;
  readonly out: readonly [string, string, string];
}

export interface CaseResult {
  readonly sessionId: string;
  readonly runOutcome: string;
  readonly runtime: string;
  readonly provider: string;
  readonly probes: Probes;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly eventTypeSet: readonly string[];
  readonly eventSource: string;
  readonly eventListError: string | null;
  readonly fallbackEventCount: number;
  readonly listedEventCount: number | null;
  readonly toolCallStartCount: number;
  readonly toolCallResultCount: number;
  readonly notificationKinds: readonly string[];
  readonly skillLoadedNames: readonly string[];
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly fileCount: number;
  readonly fileSource: string;
  readonly fileListError: string | null;
  readonly fallbackFileCount: number;
  readonly listedFileCount: number | null;
  readonly files: readonly { filename: string | null; sizeBytes: number; sample: string | null }[];
  readonly outProbesFound: readonly string[];
  readonly channelProbeSources: Readonly<Record<string, readonly string[]>>;
  readonly channelProbeMisses: readonly string[];
  readonly leakedApiKey: boolean;
  // Full payload of every runner-sourced stream_error. This captures the
  // exception message and phase when materialize / manifest fetch fails,
  // right before a runner_error terminal.
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
}

// Required event vocabulary for a successful managed run.
const EXPECTED_SUCCESS_EVENT_TYPES = [
  "RUN_STARTED",
  "TEXT_MESSAGE_CONTENT",
  "TOOL_CALL_START",
  "TOOL_CALL_RESULT",
  "CUSTOM",
  "RUN_FINISHED"
] as const;

export function dumpCase(result: CaseResult): string {
  const lines: string[] = [];
  lines.push(`sessionId=${result.sessionId} runtime=${result.runtime} provider=${result.provider}`);
  lines.push(`runOutcome=${result.runOutcome} terminalKind=${result.terminalKind}`);
  lines.push(`terminalData=${JSON.stringify(result.terminalData)}`);
  lines.push(`eventTypeSet=[${result.eventTypeSet.join(", ")}]`);
  lines.push(
    `eventSource=${result.eventSource} fallbackEvents=${result.fallbackEventCount} ` +
      `listedEvents=${result.listedEventCount ?? "(not listed)"} eventListError=${result.eventListError ?? "(none)"}`
  );
  lines.push(
    `fileSource=${result.fileSource} fallbackFiles=${result.fallbackFileCount} ` +
      `listedFiles=${result.listedFileCount ?? "(not listed)"} fileListError=${result.fileListError ?? "(none)"}`
  );
  lines.push(
    `toolCallStart=${result.toolCallStartCount} toolCallResult=${result.toolCallResultCount} ` +
      `assistantText=${result.assistantTextEventCount} events=${result.eventCount}`
  );
  lines.push(`notificationKinds=[${result.notificationKinds.join(", ")}]`);
  lines.push(`skillLoadedNames=[${result.skillLoadedNames.join(", ")}]`);
  lines.push(`channelProbeSources=${JSON.stringify(result.channelProbeSources)}`);
  lines.push(`channelProbeMisses=[${result.channelProbeMisses.join(", ")}]`);
  lines.push(`outProbesFound=[${result.outProbesFound.join(", ")}] of [${result.probes.out.join(", ")}]`);
  if (result.streamErrors.length > 0) {
    lines.push(`streamErrors:`);
    for (const se of result.streamErrors) {
      lines.push(`  - ${JSON.stringify(se).slice(0, 800)}`);
    }
  }
  lines.push(`files=${result.files.map((o) => `${o.filename}(${o.sizeBytes}B)`).join(", ")}`);
  const downloadErrors = result.files.filter((o) => o.sample?.startsWith("(download error"));
  if (downloadErrors.length > 0) {
    lines.push("fileDownloadErrors:");
    for (const o of downloadErrors) {
      lines.push(`  - ${o.filename ?? "(unknown)"}: ${(o.sample ?? "").slice(0, 800)}`);
    }
  }
  for (const o of result.files) {
    if (o.filename && o.filename.startsWith(".runtime/")) {
      lines.push(`--- ${o.filename} (sample, first 256 bytes) ---`);
      lines.push(o.sample ?? "(empty)");
      lines.push(`--- end ${o.filename} ---`);
    }
  }
  lines.push(`assistantTextJoined=${result.assistantTextJoined.slice(0, 1000)}`);
  return lines.join("\n");
}

function fail(result: CaseResult, message: string): never {
  throw new Error(`${message}\n\n${dumpCase(result)}`);
}

export function assertManagedShape(result: CaseResult, expectedSkillPrefixes: readonly [string, string, string]): void {
  if (result.runOutcome !== "succeeded") {
    fail(result, `expected runOutcome "succeeded" but got "${result.runOutcome}"`);
  }
  if (result.terminalKind !== "RUN_FINISHED") {
    fail(result, `expected terminal kind "RUN_FINISHED" but got "${result.terminalKind}"`);
  }

  if (
    !result.eventKinds.includes("RUN_STARTED") ||
    !result.eventKinds.includes("RUN_FINISHED") ||
    result.eventKinds.indexOf("RUN_STARTED") >= result.eventKinds.lastIndexOf("RUN_FINISHED")
  ) {
    fail(result, `RUN_STARTED must precede RUN_FINISHED`);
  }

  const terminal = result.terminalData ?? {};
  if (terminal["outcome"] !== "succeeded") {
    fail(result, `expected terminal outcome "succeeded" but got "${terminal["outcome"]}"`);
  }
  const checkpoint = terminal["checkpoint"];
  if (!checkpoint || typeof checkpoint !== "object" || typeof (checkpoint as Record<string, unknown>)["checkpointId"] !== "string") {
    fail(result, `RUN_FINISHED did not carry a committed checkpoint`);
  }
  if (typeof terminal["costUsd"] !== "number" || terminal["costUsd"] < 0) {
    fail(result, `RUN_FINISHED did not carry a non-negative per-run costUsd`);
  }
  if (!Array.isArray(terminal["providerUsage"])) {
    fail(result, `RUN_FINISHED did not carry per-run providerUsage`);
  }

  for (const type of EXPECTED_SUCCESS_EVENT_TYPES) {
    if (!result.eventTypeSet.includes(type)) {
      fail(result, `expected event type "${type}" was not observed`);
    }
  }

  if (result.toolCallStartCount <= 0 || result.toolCallResultCount <= 0) {
    fail(
      result,
      `expected tool-call events (start>0 && result>0) but got start=${result.toolCallStartCount} result=${result.toolCallResultCount}`
    );
  }

  if (result.skillLoadedNames.length < expectedSkillPrefixes.length) {
    fail(result, `expected at least ${expectedSkillPrefixes.length} skill_loaded events, got ${result.skillLoadedNames.length}`);
  }
  for (const prefix of expectedSkillPrefixes) {
    if (!result.skillLoadedNames.some((n) => n.startsWith(prefix))) {
      fail(result, `skill "${prefix}" produced no skill_loaded event`);
    }
  }

  if (result.assistantTextEventCount <= 0) {
    fail(result, `expected at least one assistant text event`);
  }
  if (result.assistantTextJoined.length <= 0) {
    fail(result, `expected non-empty assistant text`);
  }

  if (result.channelProbeMisses.length > 0) {
    const probeByChannel: Record<string, string> = {
      system: result.probes.system,
      instructions: result.probes.instructions,
      prompt: result.probes.prompt
    };
    const missing = result.channelProbeMisses.map((channel) => `${channel}=${probeByChannel[channel] ?? "(unknown)"}`);
    fail(result, `channel probes did not round-trip in the event transcript: ${missing.join(", ")}`);
  }

  for (const out of result.files) {
    if (out.sizeBytes < 0) {
      fail(result, `file ${out.filename ?? "(unknown)"} reported negative size ${out.sizeBytes}`);
    }
    if (out.sample !== null && out.sample.startsWith("(download error")) {
      fail(result, `file ${out.filename ?? "(unknown)"} failed to download`);
    }
  }
  if (result.outProbesFound.length < 1) {
    fail(result, `no agent-written session file carried any expected REF-out token; files pipeline unverified`);
  }

  if (result.leakedApiKey) {
    fail(result, `workspace API key leaked into SDK-visible payload`);
  }
}
