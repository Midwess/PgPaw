create table if not exists projects (
  id text primary key,
  name text not null
);

create table if not exists users (
  id text primary key,
  name text not null
);

create table if not exists todos (
  id text primary key,
  title text not null,
  completed boolean not null default false,
  project_id text not null references projects(id),
  assignee_id text references users(id)
);

grant select on projects, users, todos to public;

insert into projects (id, name) values ('p1', 'Launch'), ('p2', 'Backlog')
  on conflict do nothing;
insert into users (id, name) values ('u1', 'Ada'), ('u2', 'Linus')
  on conflict do nothing;

-- After `pgpaw init`, publish all three tables:
-- alter publication cache_server_pub add table projects, users, todos;
