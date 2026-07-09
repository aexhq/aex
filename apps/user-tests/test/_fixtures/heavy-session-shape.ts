export interface Probes {
  readonly system: string;
  readonly agentsMd: string;
  readonly prompt: string;
  readonly out: readonly [string, string, string];
}

export interface CaseResult {
  readonly sessionId: string;
  readonly attempts: number;
  readonly sessionStatus: string;
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
  readonly outputCount: number;
  readonly outputSource: string;
  readonly outputListError: string | null;
  readonly fallbackOutputCount: number;
  readonly listedOutputCount: number | null;
  readonly outputs: readonly { filename: string | null; sizeBytes: number; sample: string | null }[];
  readonly outProbesFound: readonly string[];
  readonly channelProbeSources: Readonly<Record<string, readonly string[]>>;
  readonly channelProbeMisses: readonly string[];
  readonly retryReasons: readonly string[];
  readonly leakedDeepseekKey: boolean;
  // Full payload of every sessionner-sourced stream_error. This captures the
  // exception message and phase when materialize / manifest fetch fails,
  // right before a runner_error terminal.
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
}

// Required event vocabulary for a successful managed session. Legacy
// TURN_STARTED/TURN_FINISHED frames are still accepted when present, but the
// session event log can validly expose CUSTOM aex.session.* terminals without
// a TURN_STARTED event.
const EXPECTED_SUCCESS_EVENT_TYPES = [
  "TEXT_MESSAGE_CONTENT",
  "TOOL_CALL_START",
  "TOOL_CALL_RESULT",
  "CUSTOM"
] as const;

const SUCCESS_TERMINAL_KINDS = ["TURN_FINISHED", "aex.session.idle", "aex.session.succeeded"] as const;

export function dumpCase(result: CaseResult): string {
  const lines: string[] = [];
  lines.push(`sessionId=${result.sessionId} runtime=${result.runtime} provider=${result.provider}`);
  lines.push(`sessionStatus=${result.sessionStatus} terminalKind=${result.terminalKind} attempts=${result.attempts}`);
  lines.push(`terminalData=${JSON.stringify(result.terminalData)}`);
  lines.push(`eventTypeSet=[${result.eventTypeSet.join(", ")}]`);
  lines.push(
    `eventSource=${result.eventSource} fallbackEvents=${result.fallbackEventCount} ` +
      `listedEvents=${result.listedEventCount ?? "(not listed)"} eventListError=${result.eventListError ?? "(none)"}`
  );
  lines.push(
    `outputSource=${result.outputSource} fallbackOutputs=${result.fallbackOutputCount} ` +
      `listedOutputs=${result.listedOutputCount ?? "(not listed)"} outputListError=${result.outputListError ?? "(none)"}`
  );
  lines.push(
    `toolCallStart=${result.toolCallStartCount} toolCallResult=${result.toolCallResultCount} ` +
      `assistantText=${result.assistantTextEventCount} events=${result.eventCount}`
  );
  lines.push(`notificationKinds=[${result.notificationKinds.join(", ")}]`);
  lines.push(`skillLoadedNames=[${result.skillLoadedNames.join(", ")}]`);
  lines.push(`channelProbeSources=${JSON.stringify(result.channelProbeSources)}`);
  lines.push(`channelProbeMisses=[${result.channelProbeMisses.join(", ")}] retryReasons=[${result.retryReasons.join(", ")}]`);
  lines.push(`outProbesFound=[${result.outProbesFound.join(", ")}] of [${result.probes.out.join(", ")}]`);
  if (result.streamErrors.length > 0) {
    lines.push(`streamErrors:`);
    for (const se of result.streamErrors) {
      lines.push(`  - ${JSON.stringify(se).slice(0, 800)}`);
    }
  }
  lines.push(`outputs=${result.outputs.map((o) => `${o.filename}(${o.sizeBytes}B)`).join(", ")}`);
  for (const o of result.outputs) {
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
  if (result.sessionStatus !== "succeeded") {
    fail(result, `expected sessionStatus "succeeded" but got "${result.sessionStatus}"`);
  }
  if (!SUCCESS_TERMINAL_KINDS.some((kind) => kind === result.terminalKind)) {
    fail(result, `expected success terminal kind but got "${result.terminalKind}"`);
  }

  if (result.terminalKind === "TURN_FINISHED") {
    if (!result.eventKinds.includes("TURN_STARTED")) {
      fail(result, `legacy TURN_FINISHED stream did not include TURN_STARTED`);
    }
    if (!result.eventKinds.includes("TURN_FINISHED")) {
      fail(result, `legacy TURN_FINISHED stream did not include TURN_FINISHED`);
    }
    if (result.eventKinds.indexOf("TURN_STARTED") >= result.eventKinds.lastIndexOf("TURN_FINISHED")) {
      fail(result, `legacy TURN_FINISHED stream had TURN_STARTED after TURN_FINISHED`);
    }
  } else if (!result.eventKinds.includes("CUSTOM")) {
    fail(result, `managed session terminal did not include a CUSTOM lifecycle event`);
  }

  const terminal = result.terminalData ?? {};
  if (terminal["reason"] !== "complete") {
    fail(result, `expected terminal reason "complete" but got "${terminal["reason"]}"`);
  }
  const exitCode = terminal["runtimeExitCode"];
  if (exitCode !== undefined && exitCode !== 0) {
    fail(result, `runtimeExitCode=${exitCode}`);
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
      agentsMd: result.probes.agentsMd,
      prompt: result.probes.prompt
    };
    const missing = result.channelProbeMisses.map((channel) => `${channel}=${probeByChannel[channel] ?? "(unknown)"}`);
    fail(result, `channel probes did not round-trip in the event transcript: ${missing.join(", ")}`);
  }

  for (const out of result.outputs) {
    if (out.sizeBytes < 0) {
      fail(result, `output ${out.filename ?? "(unknown)"} reported negative size ${out.sizeBytes}`);
    }
    if (out.sample !== null && out.sample.startsWith("(download error")) {
      fail(result, `output ${out.filename ?? "(unknown)"} failed to download`);
    }
  }
  if (result.outProbesFound.length < 1) {
    fail(result, `no agent-written output file carried any expected REF-out token; outputs pipeline unverified`);
  }

  if (result.leakedDeepseekKey) {
    fail(result, `DeepSeek key leaked into SDK-visible payload`);
  }
}
