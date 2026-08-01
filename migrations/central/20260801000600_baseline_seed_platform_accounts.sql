-- aex-migration: 20260801000600 baseline_seed_platform_accounts | tx=yes | destructive=no | phase=baseline

INSERT INTO finance.account (account_id, org_id, kind, normal_side)
VALUES
  ('00000000-0000-7000-8000-000000000001', NULL, 'provider_clearing', 'debit'),
  ('00000000-0000-7000-8000-000000000002', NULL, 'processor_fee_expense', 'debit'),
  ('00000000-0000-7000-8000-000000000003', NULL, 'usage_revenue', 'credit'),
  ('00000000-0000-7000-8000-000000000004', NULL, 'tax_payable', 'credit'),
  ('00000000-0000-7000-8000-000000000005', NULL, 'refund_clearing', 'debit'),
  ('00000000-0000-7000-8000-000000000006', NULL, 'dispute_holding', 'debit'),
  ('00000000-0000-7000-8000-000000000007', NULL, 'dispute_loss_expense', 'debit'),
  ('00000000-0000-7000-8000-000000000008', NULL, 'goodwill_expense', 'debit');

INSERT INTO finance.account_balance (account_id, org_id, kind, currency)
SELECT account_id, org_id, kind, currency FROM finance.account;
