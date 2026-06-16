import { execFileSync, spawn, type ChildProcess } from "node:child_process"
import { rmSync } from "node:fs"
import { resolve } from "node:path"

import { DATA_DIR, JWT_SECRET, PG_CONTAINER, PG_PORT, PGPAW_PORT, PGPAW_URL } from "./env"

const BIN = resolve(process.cwd(), "../../target/release/pgpaw")

const FIXTURES = `
create table if not exists items (id text primary key, name text not null, n int not null default 0);
grant select on items to public;

create table if not exists orgs (id int primary key, name text);
insert into orgs values (1,'Acme'),(2,'Globex') on conflict do nothing;
grant select on orgs to public;

do $$ begin if not exists (select from pg_roles where rolname='member') then create role member login; end if; end $$;

create table if not exists documents (id int primary key, org_id int references orgs(id), title text);
insert into documents values (101,1,'A-one'),(102,1,'A-two'),(201,2,'B-one'),(202,2,'B-two'),(203,2,'B-three') on conflict do nothing;
grant select on documents to member;
alter table documents enable row level security;
alter table documents force row level security;
drop policy if exists documents_by_org on documents;
create policy documents_by_org on documents for select to member
  using ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );

create table if not exists projects (id text primary key, name text not null);
insert into projects values ('p1','Launch'),('p2','Backlog') on conflict do nothing;
create table if not exists users (id text primary key, name text not null);
insert into users values ('u1','Ada'),('u2','Linus') on conflict do nothing;
create table if not exists todos (id text primary key, title text not null, completed boolean not null default false,
  project_id text references projects(id), assignee_id text references users(id));
grant select on projects, users, todos to public;
`

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

function run(cmd: string, args: string[], input?: string): string {
  return execFileSync(cmd, args, { encoding: "utf8", input, stdio: ["pipe", "pipe", "pipe"] })
}

async function waitHealth(timeoutMs: number, serve: ChildProcess, stderr: () => string): Promise<void> {
  const start = Date.now()
  for (;;) {
    if (serve.exitCode !== null) {
      throw new Error(`pgpaw serve exited (${serve.exitCode}) before healthy:\n${stderr()}`)
    }
    try {
      const r = await fetch(`${PGPAW_URL}/healthz`)
      if (r.ok && (await r.json()).status === "ok") return
    } catch {
      // not up yet
    }
    if (Date.now() - start > timeoutMs) throw new Error(`pgpaw not healthy in ${timeoutMs}ms:\n${stderr()}`)
    await sleep(500)
  }
}

export default async function setup(): Promise<() => Promise<void>> {
  try {
    run("docker", ["rm", "-f", PG_CONTAINER])
  } catch {
    // no prior container
  }
  rmSync(DATA_DIR, { recursive: true, force: true })

  run("docker", [
    "run", "-d", "--name", PG_CONTAINER,
    "-e", "POSTGRES_PASSWORD=postgres", "-e", "POSTGRES_DB=myapp",
    "-p", `${PG_PORT}:5432`, "postgres:16", "-c", "wal_level=logical",
  ])

  for (let i = 0; i < 40; i++) {
    try {
      run("docker", ["exec", PG_CONTAINER, "pg_isready", "-U", "postgres"])
      break
    } catch {
      await sleep(1000)
    }
  }
  await sleep(800)

  run("docker", ["exec", "-i", PG_CONTAINER, "psql", "-U", "postgres", "-d", "myapp", "-v", "ON_ERROR_STOP=1"], FIXTURES)

  run(BIN, [
    "init", "--pg-host", "127.0.0.1", "--pg-port", String(PG_PORT),
    "--pg-user", "postgres", "--pg-password", "postgres", "--pg-database", "myapp",
  ], "y\n")

  let err = ""
  const serve = spawn(BIN, [
    "serve", "--pg-host", "127.0.0.1", "--pg-port", String(PG_PORT),
    "--pg-user", "postgres", "--pg-password", "postgres", "--pg-database", "myapp",
    "--host", "127.0.0.1", "--port", String(PGPAW_PORT), "--data-dir", DATA_DIR,
    "--jwt-secret", JWT_SECRET,
  ], { stdio: ["ignore", "ignore", "pipe"], detached: true })
  serve.stderr?.on("data", (chunk: Buffer) => (err += chunk.toString()))

  await waitHealth(60000, serve, () => err)

  serve.stderr?.removeAllListeners("data")
  serve.stderr?.destroy()
  serve.unref()

  return async () => {
    try {
      if (serve.pid) process.kill(-serve.pid, "SIGKILL")
    } catch {
      // already gone
    }
    try {
      run("docker", ["rm", "-f", PG_CONTAINER])
    } catch {
      // already gone
    }
  }
}
