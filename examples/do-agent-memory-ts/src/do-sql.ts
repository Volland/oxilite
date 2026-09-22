// A D1-shaped view of a Durable Object's embedded SQLite, so @oxilite/d1 runs on it unchanged.
//
// The oxilite core is sans-IO: it only asks for "run these statements, atomically if asked".
// @oxilite/d1 needs two calls from its database (`prepare(sql).raw()` and `batch(stmts)`), and
// both map directly onto `ctx.storage.sql`. An atomic batch becomes `transactionSync`, the
// Durable Object's transaction.
import type { D1DatabaseLike } from "@oxilite/d1";

interface Statement {
  sql: string;
  raw(): Promise<unknown[][]>;
}

export function durableObjectDatabase(storage: DurableObjectStorage): D1DatabaseLike {
  const exec = (sql: string) => storage.sql.exec(sql).raw().toArray() as unknown[][];
  const totalChanges = () => Number(exec("SELECT total_changes()")[0][0]);
  return {
    prepare(sql: string): Statement {
      return { sql, raw: async () => exec(sql) };
    },
    async batch(statements) {
      return storage.transactionSync(() =>
        (statements as Statement[]).map((s) => {
          const before = totalChanges();
          const rows = exec(s.sql);
          const changes = totalChanges() - before;
          // The driver reads each row with Object.values(), which returns an array's items in order.
          return { results: rows as unknown as Record<string, unknown>[], meta: { changes } };
        }),
      );
    },
  };
}
