export interface PreCreateTransportObservation {
  readonly runId: string | null;
  readonly threw: string | null;
}

const PRE_CREATE_TRANSPORT_RE =
  /\b(ConnectionRefused|ECONNABORTED|ECONNRESET|ECONNREFUSED|EAI_AGAIN|ENETDOWN|ENETRESET|ENETUNREACH|ETIMEDOUT|UND_ERR_[A-Z0-9_]+)\b|socket connection was closed unexpectedly|fetch failed|terminated|unable to connect/i;

export function isPreCreateTransportFailure(observation: PreCreateTransportObservation): boolean {
  return observation.runId === null && observation.threw !== null && isPreCreateTransportMessage(observation.threw);
}

export function isPreCreateTransportMessage(message: string): boolean {
  return PRE_CREATE_TRANSPORT_RE.test(message);
}
