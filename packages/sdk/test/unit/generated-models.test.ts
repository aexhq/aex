import { describe, expect, test } from "bun:test";

import {
  type Aex,
  isId,
  newId,
  type Id,
  type MessageSendRequest,
  type MessageSendResult,
  type Session,
  type SessionCreateRequest,
} from "../../src/index.js";

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2)
    ? (<Value>() => Value extends Right ? 1 : 2) extends
      (<Value>() => Value extends Left ? 1 : 2)
      ? true
      : false
    : false;

describe("generated public models", () => {
  test("uses authored request and success types with canonical UUIDv7 constructors", () => {
    const operationId: Id<"operation"> = newId("operation");
    const request: SessionCreateRequest = {
      provider: "openai",
      model: "gpt-5",
      providerApiKey: "sk-live-fixture",
    };
    type CreateParams = Parameters<Aex["sessions"]["sessionCreate"]>[0];
    type MessageParams = Parameters<Aex["sessions"]["sessionMessageSend"]>[0];
    const createParams: CreateParams = {
      body: request,
      idempotencyKey: "create-once",
    };
    const messageParams: MessageParams = {
      sessionId: newId("session"),
      body: { text: "hello" },
      idempotencyKey: "send-once",
    };
    type CreateResult = Awaited<ReturnType<Aex["sessions"]["sessionCreate"]>>;
    type SendResult = Awaited<ReturnType<Aex["sessions"]["sessionMessageSend"]>>;
    const createBodyIsAuthored: Equal<CreateParams["body"], SessionCreateRequest> = true;
    const messageBodyIsAuthored: Equal<MessageParams["body"], MessageSendRequest> = true;
    const createResultIsAuthored: Equal<CreateResult, Session> = true;
    const sendResultIsAuthored: Equal<SendResult, MessageSendResult> = true;

    expect(isId("operation", operationId)).toBeTrue();
    expect(request.providerApiKey).toBe("sk-live-fixture");
    expect(createParams.body).toBe(request);
    expect(messageParams.body.text).toBe("hello");
    expect(createBodyIsAuthored).toBeTrue();
    expect(messageBodyIsAuthored).toBeTrue();
    expect(createResultIsAuthored).toBeTrue();
    expect(sendResultIsAuthored).toBeTrue();
  });
});
