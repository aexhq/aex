const environmentBrand: unique symbol = Symbol("environment");

export const linux = Object.freeze({
  amd64: "linux-amd64" as const,
  arm64: "linux-arm64" as const,
});

export type ComputerPlatform = (typeof linux)[keyof typeof linux];
export type NetworkEnforcement = "none" | "allowlist" | "unrestricted";
export type RecoveryBehavior = "retained" | "connection" | "replay-safe";
type WireRecoveryBehavior = "retained" | "connection" | "replay_safe";

export interface ComputerProfile {
  readonly kind: "computer";
  readonly platform: ComputerPlatform;
  readonly network: NetworkEnforcement;
  readonly recovery: RecoveryBehavior;
  readonly workspace: true;
  readonly processes: true;
  readonly streaming: true;
  readonly setup: true;
}

export interface CallbacksProfile {
  readonly kind: "callbacks";
  readonly network: "unrestricted";
  readonly recovery: "connection" | "replay-safe";
  readonly workspace: false;
  readonly processes: false;
  readonly streaming: true;
  readonly setup: false;
}

export type EnvironmentProfile = ComputerProfile | CallbacksProfile;

export function computer(options: {
  platform: ComputerPlatform;
  network: NetworkEnforcement;
  recovery: "retained";
}): ComputerProfile {
  return Object.freeze({
    kind: "computer",
    platform: options.platform,
    network: options.network,
    recovery: options.recovery,
    workspace: true,
    processes: true,
    streaming: true,
    setup: true,
  });
}

export function callbacks(options: {
  recovery?: "connection" | "replay-safe";
} = {}): CallbacksProfile {
  return Object.freeze({
    kind: "callbacks",
    network: "unrestricted",
    recovery: options.recovery ?? "connection",
    workspace: false,
    processes: false,
    streaming: true,
    setup: false,
  });
}

export interface EnvironmentHandleContext {
  readonly sessionId: string;
  readonly environment: string;
  request<T>(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<T>;
}

export interface EnvironmentRef<
  Identity extends string = string,
  Profile extends EnvironmentProfile = EnvironmentProfile,
  Handle = unknown,
> {
  readonly [environmentBrand]: {
    readonly identity: Identity;
    readonly profile: Profile;
    readonly handle: Handle;
  };
}

export type HandleOf<Environment> = Environment extends EnvironmentRef<string, EnvironmentProfile, infer Handle>
  ? Handle
  : never;
export type ProfileOf<Environment> = Environment extends EnvironmentRef<string, infer Profile, unknown>
  ? Profile
  : never;

export interface EnvironmentDefinition<
  Options,
  Handle,
  Identity extends string,
  Profile extends EnvironmentProfile,
> {
  readonly identity: Identity;
  readonly protocol: "environment/v1";
  readonly profile: Profile;
  serialize(options: Options): Readonly<Record<string, unknown>>;
  handle(context: EnvironmentHandleContext, options: Options): Handle;
}

interface Descriptor {
  readonly identity: string;
  readonly protocol: "environment/v1";
  readonly profile: EnvironmentProfile;
  readonly configuration: Readonly<Record<string, unknown>>;
  createHandle(context: EnvironmentHandleContext): unknown;
}

const descriptors = new WeakMap<object, Descriptor>();

export type EnvironmentFactory<
  Options,
  Handle,
  Identity extends string,
  Profile extends EnvironmentProfile,
> = [Options] extends [void]
  ? (options?: void) => EnvironmentRef<Identity, Profile, Handle>
  : (options: Options) => EnvironmentRef<Identity, Profile, Handle>;

export function defineEnvironment<
  Options = void,
  Handle = unknown,
  const Identity extends string = string,
  const Profile extends EnvironmentProfile = EnvironmentProfile,
>(definition: EnvironmentDefinition<Options, Handle, Identity, Profile>): EnvironmentFactory<Options, Handle, Identity, Profile> {
  if (definition.identity.trim() === "") throw new TypeError("Environment identity cannot be empty");
  if (definition.protocol !== "environment/v1") throw new TypeError("Unsupported environment protocol");
  return ((options?: Options) => {
    const configuration = definition.serialize(options as Options);
    assertPlainObject(configuration, "Environment configuration");
    const ref = Object.freeze(Object.create(null)) as EnvironmentRef<Identity, Profile, Handle>;
    descriptors.set(ref, Object.freeze({
      identity: definition.identity,
      protocol: definition.protocol,
      profile: definition.profile,
      configuration: deepFreeze({ ...configuration }),
      createHandle: (context: EnvironmentHandleContext) => definition.handle(context, options as Options),
    }));
    return ref;
  }) as EnvironmentFactory<Options, Handle, Identity, Profile>;
}

export interface SerializedEnvironment {
  readonly extension: string;
  readonly protocol: "environment/v1";
  readonly profile: {
    readonly kind: EnvironmentProfile["kind"];
    readonly platform?: ComputerPlatform;
    readonly network: NetworkEnforcement;
    readonly recovery: WireRecoveryBehavior;
  };
  readonly configuration: Readonly<Record<string, unknown>>;
}

export function inspectEnvironment(environment: EnvironmentRef): {
  readonly serialized: SerializedEnvironment;
  createHandle(context: EnvironmentHandleContext): unknown;
} {
  const descriptor = descriptors.get(environment as object);
  if (descriptor === undefined) throw new TypeError("Value is not an EnvironmentRef");
  return {
    serialized: Object.freeze({
      extension: descriptor.identity,
      protocol: descriptor.protocol,
      profile: Object.freeze({
        kind: descriptor.profile.kind,
        ...(descriptor.profile.kind === "computer" ? { platform: descriptor.profile.platform } : {}),
        network: descriptor.profile.network,
        recovery: descriptor.profile.recovery === "replay-safe"
          ? "replay_safe"
          : descriptor.profile.recovery,
      }),
      configuration: descriptor.configuration,
    }),
    createHandle: descriptor.createHandle,
  };
}

export function isEnvironmentRef(value: unknown): value is EnvironmentRef {
  return typeof value === "object" && value !== null && descriptors.has(value);
}

function assertPlainObject(value: unknown, label: string): asserts value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${label} must be a plain object`);
  }
  const prototype = Object.getPrototypeOf(value) as unknown;
  if (prototype !== Object.prototype && prototype !== null) {
    throw new TypeError(`${label} must be a plain object`);
  }
}

function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === "object" && !Object.isFrozen(value)) {
    for (const child of Object.values(value as Record<string, unknown>)) deepFreeze(child);
    Object.freeze(value);
  }
  return value;
}
