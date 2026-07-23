/**
 * In-memory fakes for the custody / retention manifest object stores — moved
 * out of the production modules (`session-custody.ts` / `session-retention.ts`)
 * so production `src/` carries zero test doubles. Both enforce the same
 * public-safety assertion the real stores rely on, so a leaking payload fails
 * in tests exactly as it would at the boundary.
 */
import {
  assertPublicSafeCustodyPayload,
  custodyManifestObjectKey,
  type CustodyManifestObjectStore,
  type CustodyManifestV1,
  type CustodyManifestWriteObject
} from "../session-custody.js";
import {
  assertPublicSafeSessionRetentionPayload,
  type SessionDeletionManifestObjectStore,
  type SessionDeletionManifestV1,
  type SessionDeletionManifestWriteObject
} from "../session-retention.js";

export class FakeCustodyManifestObjectStore implements CustodyManifestObjectStore {
  #objects = new Map<string, CustodyManifestV1>();

  async putCustodyManifestObject(object: CustodyManifestWriteObject): Promise<void> {
    assertPublicSafeCustodyPayload(object.manifest);
    this.#objects.set(object.key, cloneJson(object.manifest));
  }

  getBySessionId(sessionId: string): CustodyManifestV1 | undefined {
    return this.get(custodyManifestObjectKey(sessionId));
  }

  get(key: string): CustodyManifestV1 | undefined {
    const object = this.#objects.get(key);
    return object ? cloneJson(object) : undefined;
  }

  listKeys(): readonly string[] {
    return Object.freeze([...this.#objects.keys()].sort());
  }
}

export class FakeSessionDeletionManifestObjectStore implements SessionDeletionManifestObjectStore {
  #objects = new Map<string, SessionDeletionManifestV1>();

  async putSessionDeletionManifestObject(object: SessionDeletionManifestWriteObject): Promise<void> {
    assertPublicSafeSessionRetentionPayload(object.manifest);
    this.#objects.set(object.sessionId, cloneJson(object.manifest));
  }

  getBySessionId(sessionId: string): SessionDeletionManifestV1 | undefined {
    return this.get(sessionId);
  }

  get(sessionId: string): SessionDeletionManifestV1 | undefined {
    const object = this.#objects.get(sessionId);
    return object ? cloneJson(object) : undefined;
  }

  listSessionIds(): readonly string[] {
    return Object.freeze([...this.#objects.keys()].sort());
  }
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}
