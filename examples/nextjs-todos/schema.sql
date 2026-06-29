create table if not exists todos (
  id text primary key,
  title text not null,
  completed boolean not null default false
);

grant select on todos to public;

-- After `pgpaw init`, make sure the table is in the publication PgPaw reads:
-- alter publication pgpaw_pub add table todos;
