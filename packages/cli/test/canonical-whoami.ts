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
      llmTokenAllowanceRemainingUsd: 2,
      creditGateActive: true,
      paymentMethodStatus: "none" as const,
      admissionState: "free" as const,
      autoTopupEnabled: false,
      accountType: "standard" as const
    }
  };
}
