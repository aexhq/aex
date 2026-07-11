import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

export interface WorkflowStep {
  readonly name?: string;
  readonly uses?: string;
  readonly run?: string;
  readonly if?: string;
  readonly env?: Readonly<Record<string, unknown>>;
  readonly with?: Readonly<Record<string, unknown>>;
  readonly [key: string]: unknown;
}

export interface WorkflowJob {
  readonly name?: string;
  readonly needs?: string | readonly string[];
  readonly if?: string;
  readonly env?: Readonly<Record<string, unknown>>;
  readonly steps?: readonly WorkflowStep[];
  readonly strategy?: {
    readonly "fail-fast"?: boolean;
    readonly "max-parallel"?: number;
    readonly matrix?: Readonly<Record<string, unknown>>;
  };
  readonly [key: string]: unknown;
}

export interface WorkflowDocument {
  readonly on?: string | readonly string[] | Readonly<Record<string, unknown>>;
  readonly permissions?: string | Readonly<Record<string, unknown>>;
  readonly env?: Readonly<Record<string, unknown>>;
  readonly jobs: Readonly<Record<string, WorkflowJob>>;
  readonly [key: string]: unknown;
}

export function readRepoFile(path: string): string {
  return readFileSync(resolve(repoRoot, path), "utf8").replace(/\r\n/g, "\n");
}

export function readWorkflow(path: string): WorkflowDocument {
  const value: unknown = parse(readRepoFile(path));
  if (!isRecord(value) || !isRecord(value.jobs)) {
    throw new Error(`${path} must contain a jobs mapping`);
  }
  return value as unknown as WorkflowDocument;
}

export function workflowTriggers(workflow: WorkflowDocument): Readonly<Record<string, unknown>> {
  if (!isRecord(workflow.on)) {
    throw new Error("workflow must declare triggers as a mapping");
  }
  return workflow.on;
}

export function workflowJob(workflow: WorkflowDocument, id: string): WorkflowJob {
  const job = workflow.jobs[id];
  if (!job) throw new Error(`workflow job ${id} is missing`);
  return job;
}

export function workflowStep(job: WorkflowJob, name: string): WorkflowStep {
  const step = job.steps?.find((candidate) => candidate.name === name);
  if (!step) throw new Error(`workflow step ${name} is missing`);
  return step;
}

export function jobNeeds(job: WorkflowJob): readonly string[] {
  if (job.needs === undefined) return [];
  return typeof job.needs === "string" ? [job.needs] : job.needs;
}

export function stepIndex(job: WorkflowJob, name: string): number {
  const index = job.steps?.findIndex((step) => step.name === name) ?? -1;
  if (index < 0) throw new Error(`workflow step ${name} is missing`);
  return index;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
