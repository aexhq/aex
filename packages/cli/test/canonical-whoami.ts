export function canonicalWhoami(workspaceId: string, scopes: readonly string[] = []) {
  return {
    ok: true as const,
    principalType: "api_key" as const,
    workspaceId,
    scopes,
    limits: {
      maxConcurrentSessions: 1,
      submitRatePerMinute: 0,
      spendCapUsd: 0,
      monthSpendUsd: 0,
      balanceUsd: 0,
      balanceGraceFloorUsd: 0,
      balanceGateActive: true,
      paymentMethodStatus: "none" as const,
      planKey: "free" as const,
      accountType: "standard" as const,
      subscriptionStatus: "none" as const,
      subscriptionGate: "ok" as const
    }
  };
}
