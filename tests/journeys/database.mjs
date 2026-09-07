import { Client } from "pg";
import { randomUUID } from "node:crypto";
import assert from "node:assert/strict";
export async function database() {
  assert.ok(process.env.AEX_TEST_DATABASE_URL, "AEX_TEST_DATABASE_URL is required");
  const schema = `journey_${randomUUID().replaceAll("-", "")}`;
  const admin = new Client({ connectionString: process.env.AEX_TEST_DATABASE_URL });
  await admin.connect();
  await admin.query(`CREATE SCHEMA ${schema}`);
  const url = new URL(process.env.AEX_TEST_DATABASE_URL);
  url.searchParams.set("options", `-csearch_path=${schema}`);
  const client = new Client({ connectionString: url.href });
  await client.connect();
  const tables = ["accounts", "api_keys", "sessions", "hosts", "claims", "storage_report", "dashboard_sessions", "artifacts"];
  return {
    url: url.href, client,
    async snapshot() {
      await client.query("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY");
      const rows = {};
      try {
        for (const table of tables) rows[table] = (await client.query(`SELECT * FROM ${table}`)).rows;
        await client.query("COMMIT"); return rows;
      } catch (error) { await client.query("ROLLBACK"); throw error; }
    },
    async restore(rows) {
      await client.query("BEGIN");
      try {
        await client.query(`TRUNCATE ${tables.join(",")}`);
        for (const table of tables) for (const row of rows[table]) {
          const columns = Object.keys(row);
          await client.query(`INSERT INTO ${table} (${columns.join(",")}) VALUES (${columns.map((_,i) => `$${i+1}`).join(",")})`, Object.values(row));
        }
        await client.query("COMMIT");
      } catch (error) { await client.query("ROLLBACK"); throw error; }
    },
    async close() { await client.end(); await admin.query(`DROP SCHEMA ${schema} CASCADE`); await admin.end(); },
  };
}
