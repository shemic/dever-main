//! Checked Model/transaction source through LLVM and the real database ABI.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm_module.rs"]
mod llvm_module;
#[cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    feature = "runtime-postgres"
))]
#[path = "dever-tests/tests/support/postgres.rs"]
pub mod postgres;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::llvm_module;
    use dever_backend_bridge::{Target, emit_object};
    use dever_core::{check, hir::Program, source::SourceMap};
    use std::time::Duration;

    const SETTING: &str = r#"{"database":{"default":{"type":"sqlite","path":"data/db/owned.db","max_connections":1,"max_page_size":10}}}"#;
    const ITEM: &str = r#"type Item {
  name: Text(1, 64) unique
  amount: Int default 1 index
  note: Text?
}
"#;

    struct Fixture<'a> {
        model: &'a str,
        app: &'a str,
        body: &'a str,
    }

    const CRUD: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"some_text(value: Text) (answer: Text?) { answer = value }
amount(value: null) (answer: Int) { answer = -1 }
amount(value: model.Item) (answer: Int) { answer = value.amount }
exercise() (okay: Bool) {
  saved = model.create({ name = "你好 🌍" })
  changed = model.update(saved.id, { amount = 7
    note = "owned" })
  loaded = model.get(saved.id)
  first = model.first({ where = name == "你好 🌍" })
  absent = model.first({ where = name == "missing" })
  page = model.list({ order = id.asc
    size = 2 })
  present = model.exists({ where = name == "你好 🌍" })
  removed = model.delete(saved.id)
  remaining = model.count()
  okay = loaded.id == saved.id and loaded.name == "你好 🌍" and loaded.amount == 7 and loaded.note == some_text("owned") and amount(first) == 7 and amount(absent) == -1 and changed == 1 and page.total == 1 and length(page.items) == 1 and present and removed == 1 and remaining == 0
}
"#,
        body: "{}",
    };

    const BULK: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"exercise() (okay: Bool) {
  inserted = model.create_many([{ name = "one"
    amount = 2 }, { name = "two"
    amount = 3 }])
  updated = model.upsert({ name = "one" }, { amount = 99 }, { amount = 4 })
  created = model.upsert({ name = "three" }, { amount = 5 }, { amount = 99 })
  changed = model.update({ where = amount >= 3 }, { note = "bulk" })
  count = model.count({ where = not contains(name, "missing") and in(amount, [3, 4, 5]) and between(amount, 3, 5) and note != null })
  removed = model.delete({ where = amount >= 3 })
  okay = inserted == 2 and updated.amount == 4 and created.amount == 5 and changed == 3 and count == 3 and removed == 3 and model.count() == 0
}
"#,
        body: "{}",
    };

    const NULL_EQUALITY: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"some_text(value: Text) (answer: Text?) { answer = value }
missing_text() (answer: Text?) { answer = null }
exercise() (okay: Bool) {
  absent = model.create({ name = "absent" })
  present = model.create({ name = "present"
    note = "value" })
  missing = missing_text()
  value = some_text("value")
  nulls = model.count({ where = note == missing })
  values = model.count({ where = note == value })
  not_nulls = model.count({ where = note != missing })
  removed = model.delete({ where = name == "absent" or name == "present" })
  okay = nulls == 1 and values == 1 and not_nulls == 1 and absent.note == null and present.note == value and removed == 2
}
"#,
        body: "{}",
    };

    const CURSOR: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"names(value: model.Item) (name: Text) { name = value.name }
cursor_after(items: List<model.Item>) (value: model.Item?) { value = first(items) }
exercise() (okay: Bool) {
  inserted = model.create_many([{ name = "one"
    amount = 1 }, { name = "two"
    amount = 1 }, { name = "three"
    amount = 2 }])
  first = model.cursor({ order = [amount.asc, id.asc]
    size = 2 })
  second = model.cursor({ after = first.next
    order = [amount.asc, id.asc]
    size = 2 })
  end = model.cursor({ after = cursor_after(second.items)
    order = [amount.asc, id.asc]
    size = 2 })
  page = model.list({ order = [amount.asc, id.asc]
    page = 2
    size = 2 })
  removed = model.delete({ where = amount >= 1 })
  okay = inserted == 3 and each(names, first.items) == ["one", "two"] and first.has_more and each(names, second.items) == ["three"] and second.next == null and not second.has_more and length(end.items) == 0 and end.next == null and each(names, page.items) == ["three"] and page.total == 3 and removed == 3
}
"#,
        body: "{}",
    };

    const SCALARS: Fixture<'static> = Fixture {
        model: r#"type State {
  Active
  Disabled
}
type Item {
  name: Text(1, 64) unique
  enabled: Bool
  amount: Int
  fraction: Float
  price: Decimal(10, 2)
  payload: Bytes(8)
  external_id: Uuid
  generated_id: Uuid generated unique
  instant: DateTime
  day: Date
  clock: Time
  elapsed: Duration
  document: Json
  state: State default State.Active
  private memo: Text?
}
"#,
        app: r#"exercise(uuid: Uuid, instant: DateTime, day: Date, clock: Time, elapsed: Duration, document: Json) (okay: Bool) {
  payload = dever.bytes.from_ints([0, 128, 255])
  saved = model.create({ name = "scalar"
    enabled = true
    amount = 9223372036854775807
    fraction = 1.5
    price = 123.45
    payload = payload
    external_id = uuid
    instant = instant
    day = day
    clock = clock
    elapsed = elapsed
    document = document
    memo = "private" })
  loaded = model.get(saved.id)
  removed = model.delete(saved.id)
  okay = loaded.enabled and loaded.amount == 9223372036854775807 and loaded.fraction == 1.5 and loaded.price == 123.45 and loaded.payload == payload and loaded.external_id == uuid and loaded.generated_id == saved.generated_id and loaded.instant == instant and loaded.day == day and loaded.clock == clock and loaded.elapsed == elapsed and loaded.document == document and loaded.state == model.State.Active and loaded.memo != null and removed == 1
}
"#,
        body: r#"{"uuid":"01890f3e-7c00-7000-8000-000000000001","instant":"2026-10-01T12:34:56.789Z","day":"2026-10-01","clock":"12:34:56.789","elapsed":-42,"document":{"large":9999999999999999999999999999,"nested":[true,null]}}"#,
    };

    const TRANSACTION: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"transaction insert() (saved: model.Item) { saved = model.create({ name = "nested" }) }
forward() (saved: model.Item) { saved = insert() }
transaction edit() (saved: model.Item) {
  inserted = forward()
  changed = model.update(inserted.id, { amount = 7 })
  saved = model.get(inserted.id)
}
exercise() (okay: Bool) {
  saved = edit()
  loaded = model.get(saved.id)
  removed = model.delete(saved.id)
  okay = saved.amount == 7 and loaded.amount == 7 and removed == 1 and model.count() == 0
}
"#,
        body: "{}",
    };

    const STREAM: Fixture<'static> = Fixture {
        model: r#"type Item {
  name: Text(1, 64) unique
  amount: Int default 1
}
seed { { name = "one"
  amount = 2 }
  { name = "two"
  amount = 3 } }
"#,
        app: r#"add(value: model.Item, previous: Int) (next: Int) { next = previous + value.amount }
one(value: model.Item, previous: Int) (next: Int, stop: Bool) { next = previous + value.amount
  stop = true }
exercise() (okay: Bool) {
  full = model.stream({ order = id.asc
    size = 1 })
  total = reduce(add, full, 0)
  partial = model.stream({ order = id.asc
    size = 1 })
  alias = partial
  first = reduce_until(one, partial, 0)
  remaining = reduce(add, alias, 0)
  unopened = model.stream({ size = 1 })
  close(unopened)
  okay = total == 5 and first == 2 and remaining == 0 and model.count() == 2
}
"#,
        body: "{}",
    };

    const SQL: Fixture<'static> = Fixture {
        model: r#"type Summary { amount: Int }
type Item {
  name: Text(1, 64) unique
  amount: Int default 1
}
sql by_name(name: Text) (item: Item?) {
  sqlite = "SELECT id, created_at, name, amount FROM item WHERE name = ?1"
  postgres = "SELECT id, created_at, name, amount FROM item WHERE name = $1"
}
sql all_items(amount: Int) (items: List<Item>) {
  sqlite = "SELECT id, created_at, name, amount FROM item WHERE amount >= ?1 ORDER BY id"
  postgres = "SELECT id, created_at, name, amount FROM item WHERE amount >= $1::bigint ORDER BY id"
}
sql summary() (summary: Summary) {
  sqlite = "SELECT SUM(amount) FROM item"
  postgres = "SELECT SUM(amount)::bigint FROM item"
}
"#,
        app: r#"amount(value: null) (answer: Int) { answer = -1 }
amount(value: model.Item) (answer: Int) { answer = value.amount }
transaction populate() (okay: Bool) {
  inserted = model.create_many([{ name = "one"
    amount = 2 }, { name = "two"
    amount = 3 }])
  found = model.by_name("one")
  absent = model.by_name("missing")
  rows = model.all_items(2)
  summary = model.summary()
  okay = inserted == 2 and amount(found) == 2 and amount(absent) == -1 and length(rows) == 2 and summary.amount == 5
}
exercise() (okay: Bool) {
  valid = populate()
  removed = model.delete({ where = amount >= 2 })
  okay = valid and removed == 2 and model.count() == 0
}
"#,
        body: "{}",
    };

    const ERRORS: &str = r#"type ReadResult {
  Done(value: model.Item)
  error Failed(error: dever.database.Error)
}
category(error: dever.database.Error.Pool(message)) (kind: Int) recover("assert database failure identity") { kind = 1 }
category(error: dever.database.Error.PoolExhausted(message)) (kind: Int) recover("assert database failure identity") { kind = 2 }
category(error: dever.database.Error.Connection(message)) (kind: Int) recover("assert database failure identity") { kind = 3 }
category(error: dever.database.Error.Timeout(message)) (kind: Int) recover("assert database failure identity") { kind = 4 }
category(error: dever.database.Error.Cancelled(message)) (kind: Int) recover("assert database failure identity") { kind = 5 }
category(error: dever.database.Error.Database(message)) (kind: Int) recover("assert database failure identity") { kind = 6 }
category(error: dever.database.Error.Constraint(message)) (kind: Int) recover("assert database failure identity") { kind = 7 }
category(error: dever.database.Error.NotFound(message)) (kind: Int) recover("assert database failure identity") { kind = 8 }
category(error: dever.database.Error.InvalidData(message)) (kind: Int) recover("assert database failure identity") { kind = 9 }
category(error: dever.database.Error.Migration(message)) (kind: Int) recover("assert database failure identity") { kind = 10 }
captured(value: ReadResult.Done(item)) (kind: Int) recover("assert database failure identity") { kind = 0 }
captured(value: ReadResult.Failed(error)) (kind: Int) recover("assert database failure identity") { kind = category(error) }
"#;

    fn captured_errors() -> Fixture<'static> {
        Fixture {
            model: ITEM,
            app: r#"transaction missing() (value: model.Item) {
  saved = model.create({ name = "temporary" })
  removed = model.delete(saved.id)
  value = model.get(saved.id)
}
transaction duplicate() (value: model.Item) {
  saved = model.create({ name = "duplicate" })
  value = model.create({ name = "duplicate" })
}
exercise() (okay: Bool) {
  missing = captured(result(missing()))
  duplicate = captured(result(duplicate()))
  retained = model.count()
  removed = model.delete({ where = name == "duplicate" })
  okay = missing == 8 and duplicate == 7 and retained == 1 and removed == 1 and model.count() == 0
}
"#,
            body: "{}",
        }
    }

    const NUMERIC_ROLLBACK: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"transaction fail_numeric(divisor: Int) (okay: Bool) {
  saved = model.create({ name = "numeric rollback" })
  okay = 42 // divisor == 42
}
exercise(divisor: Int) (okay: Bool) { okay = fail_numeric(divisor) }
"#,
        body: r#"{"divisor":0}"#,
    };

    const CAPTURED_NOT_FOUND: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"transaction missing() (value: model.Item) {
  saved = model.create({ name = "temporary" })
  removed = model.delete(saved.id)
  value = model.get(saved.id)
}
exercise() (okay: Bool) {
  missing = captured(result(missing()))
  okay = missing == 8 and model.count() == 0
}
"#,
        body: "{}",
    };

    const CONSTRAINT_ROLLBACK: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"exercise() (okay: Bool) {
  first = model.create({ name = "duplicate" })
  second = model.create({ name = "duplicate" })
  okay = true
}
inspect() (okay: Bool) { okay = model.count() == 0 }
"#,
        body: "{}",
    };

    const ENCODING_ROLLBACK: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"exercise(denominator: Float) (value: Float) {
  saved = model.create({ name = "encoding rollback" })
  value = 1.0 / denominator
}
"#,
        body: r#"{"denominator":0}"#,
    };

    const TASK_ERROR: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"missing(id: model.id) (value: model.Item) { value = model.get(id) }
task_missing(id: model.id) (value: model.Item) {
  pending = run(missing(id))
  value = wait(pending)
}
exercise(id: model.id) (okay: Bool) {
  missing = captured(result(task_missing(id)))
  okay = missing == 8 and model.count() == 0
}
"#,
        body: r#"{"id":9223372036854775807}"#,
    };

    const HANDLER_TRANSACTION: Fixture<'static> = Fixture {
        model: ITEM,
        app: r#"update(item: model.Item) (count: Int) { count = model.update(item.id, { amount = 7 }) }
transaction change_all() (count: Int) {
  inserted = model.create_many([{ name = "one" }, { name = "two" }])
  page = model.list({ order = id.asc
    size = 10 })
  count = sum(update, page.items)
}
exercise() (okay: Bool) {
  changed = change_all()
  matching = model.count({ where = amount == 7 })
  removed = model.delete({ where = amount == 7 })
  okay = changed == 2 and matching == 2 and removed == 2 and model.count() == 0
}
"#,
        body: "{}",
    };

    const SEED: Fixture<'static> = Fixture {
        model: r#"type Item {
  name: Text(1, 64) unique
  amount: Int default 1
}
seed { { name = "seed"
  amount = 2 } }
"#,
        app: r#"exercise() (okay: Bool) {
  seeded = model.count({ where = name == "seed" and amount == 2 })
  created = model.create({ name = "temporary" })
  removed = model.delete(created.id)
  okay = seeded == 1 and created.amount == 1 and removed == 1 and model.count() == 1
}
"#,
        body: "{}",
    };

    const EMPTY_BULK: Fixture<'static> = Fixture {
        model: "type Item {}",
        app: r#"exercise() (okay: Bool) {
  created = model.create_many([{}, {}])
  count = model.count()
  removed = model.delete({})
  okay = created == 2 and count == 2 and removed == 2
}
"#,
        body: "{}",
    };

    const MIGRATION_INITIAL: Fixture<'static> = Fixture {
        model: r#"type Item {
  name: Text(1, 64) unique
  amount: Int default 1
  legacy: Text?
}
seed { { name = "too-long-name"
  legacy = "old" } }
"#,
        app: r#"exercise() (okay: Bool) { okay = model.count({ where = name == "too-long-name" and amount == 1 }) == 1 }
"#,
        body: "{}",
    };

    const MIGRATION_UPGRADED: Fixture<'static> = Fixture {
        model: r#"type Item {
  display_name: Text(1, 8) from name unique
  amount: Int default 2
}
migrate normalize_names {
  before {
    sqlite = "UPDATE item SET name = ?1 WHERE name = ?2"
    postgres = "UPDATE item SET name = $1 WHERE name = $2"
    parameters = ["short", "too-long-name"]
  }
  drop legacy
  after {
    sqlite = "UPDATE item SET display_name = ?1 WHERE display_name = ?2"
    postgres = "UPDATE item SET display_name = $1 WHERE display_name = $2"
    parameters = ["final", "short"]
  }
}
seed { { display_name = "second" } }
"#,
        app: r#"exercise() (okay: Bool) {
  original = model.count({ where = display_name == "final" and amount == 1 })
  seeded = model.count({ where = display_name == "second" and amount == 2 })
  created = model.create({ display_name = "temp" })
  removed = model.delete(created.id)
  okay = original == 1 and seeded == 1 and created.amount == 2 and removed == 1 and model.count() == 2
}
"#,
        body: "{}",
    };

    const CANCEL_STREAM: Fixture<'static> = Fixture {
        model: r#"type Item { amount: Int unique }
seed { { amount = 1 }
  { amount = 2 }
  { amount = 3 }
  { amount = 4 } }
"#,
        app: r#"pause(item: model.Item, ready: Channel<Int>) (next: Channel<Int>) {
  send(ready, item.amount)
  dever.task.sleep(1000)
  next = ready
}
scan(ready: Channel<Int>) (okay: Bool) {
  rows = model.stream({ order = id.asc
    size = 1 })
  signal = reduce(pause, rows, ready)
  okay = false
}
fallback() (okay: Bool) { okay = model.count() == 4 }
exercise() (okay: Bool) {
  ready = channel(Int, 1)
  pending = run(scan(ready))
  started = receive(ready)
  released = timeout(pending, 1, fallback)
  close(ready)
  okay = started != null and released and model.count() == 4
}
"#,
        body: "{}",
    };

    fn relations() -> SourceMap {
        let mut sources = SourceMap::default();
        sources.add(
            "store/account/model/organization.dever",
            "type Organization { name: Text(1, 64) }\nrelation users = store.account.model.user.organization_id",
        );
        sources.add(
            "store/account/model/user.dever",
            "type User { name: Text(1, 64)\n organization_id: store.account.model.organization.id?\n manager_id: store.account.model.user.id? }\nrelation posts = store.account.model.post.author_id\nrelation other_posts = store.account.model.post.author_id\nrelation unloaded_posts = store.account.model.post.author_id",
        );
        sources.add(
            "store/account/model/post.dever",
            "type Post { author_id: store.account.model.user.id\n title: Text(1, 64) }",
        );
        sources.add(
            "store/account/app.dever",
            r#"organization_name(value: Related.Unloaded<model.organization.Organization?>()) (name: Text) { name = "unloaded" }
organization_name(value: Related.Loaded<model.organization.Organization?>(organization)) (name: Text) { name = nullable_organization(organization) }
nullable_organization(value: null) (name: Text) { name = "none" }
nullable_organization(value: model.organization.Organization) (name: Text) { name = value.name }
manager_name(value: Related.Unloaded<model.user.User?>()) (name: Text) { name = "unloaded" }
manager_name(value: Related.Loaded<model.user.User?>(manager)) (name: Text) { name = nullable_user(manager) }
nullable_user(value: null) (name: Text) { name = "none" }
nullable_user(value: model.user.User) (name: Text) { name = value.name }
post_count(value: Related.Unloaded<List<model.post.Post>>()) (count: Int) { count = -1 }
post_count(value: Related.Loaded<List<model.post.Post>>(posts)) (count: Int) { count = length(posts) }
is_primary(value: model.user.User) (yes: Bool) { yes = value.name == "First" }
is_secondary(value: model.user.User) (yes: Bool) { yes = value.name == "Second" }
transaction loaded() (page: model.user.Page) { page = model.user.list({ order = id.asc
  size = 10
  with = [organization, manager, posts, other_posts] }) }
exercise() (okay: Bool) {
  organization = model.organization.create({ name = "Acme" })
  primary = model.user.create({ name = "First"
    organization_id = organization.id })
  secondary = model.user.create({ name = "Second"
    manager_id = primary.id })
  inserted = model.post.create_many([{ author_id = primary.id
    title = "One" }, { author_id = primary.id
    title = "Two" }])
  unloaded = model.user.get(primary.id)
  page = loaded()
  first_loaded = find(is_primary, page.items)
  second_loaded = find(is_secondary, page.items)
  valid = validate(first_loaded, second_loaded)
  deleted_posts = model.post.delete({ where = author_id == primary.id })
  deleted_second = model.user.delete(secondary.id)
  deleted_first = model.user.delete(primary.id)
  deleted_org = model.organization.delete(organization.id)
  okay = inserted == 2 and organization_name(unloaded.organization) == "unloaded" and page.total == 2 and valid and deleted_posts == 2 and deleted_second == 1 and deleted_first == 1 and deleted_org == 1
}
validate(first: null, second: model.user.User?) (okay: Bool) { okay = false }
validate(first: model.user.User, second: null) (okay: Bool) { okay = false }
validate(first: model.user.User, second: model.user.User) (okay: Bool) { okay = organization_name(first.organization) == "Acme" and organization_name(second.organization) == "none" and manager_name(second.manager) == "First" and post_count(first.posts) == 2 and post_count(first.other_posts) == 2 and post_count(second.posts) == 0 and post_count(first.unloaded_posts) == -1 }
"#,
        );
        sources.add("store/account/api.dever", "cmd exercise = app.exercise");
        sources
    }

    fn fixture_sources(fixture: &Fixture<'_>) -> SourceMap {
        fixture_sources_with_api(fixture, "cmd exercise = app.exercise")
    }

    fn fixture_sources_with_api(fixture: &Fixture<'_>, api: &str) -> SourceMap {
        let mut sources = SourceMap::default();
        sources.add("store/item/model.dever", fixture.model);
        sources.add("store/item/app.dever", fixture.app);
        sources.add("store/item/api.dever", api);
        sources
    }

    fn checked(sources: &SourceMap) -> Program {
        check(sources).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(sources))
                    .collect::<String>()
            )
        })
    }

    fn entry_ir(sources: &SourceMap, fault: Option<u32>) -> String {
        let program = checked(sources);
        let mut ir = dever_core::llvm::emit_application(&program, sources).unwrap();
        let checks = fault.map_or_else(
            || "  %passed = icmp eq i32 %status, 0".to_owned(),
            |code| llvm_module::fault_check(code, None),
        );
        ir.push_str(&format!(
            "\ndefine i32 @dever_test_run() {{\nentry:\n  %out = alloca i8\n  %fault = alloca %dever.fault\n  %status = call i32 @dever_application_entry(ptr %out, ptr %fault)\n{checks}\n  call void @dever_outputs_release(ptr %out)\n  call void @dever_fault_release(ptr %fault)\n  %exit = select i1 %passed, i32 0, i32 1\n  ret i32 %exit\n}}\n"
        ));
        ir
    }

    fn assert_output(output: std::process::Output, fault: bool) {
        if fault {
            assert!(output.stdout.is_empty());
        } else {
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                "{\"code\":0,\"message\":\"ok\",\"data\":true}\n".repeat(65)
            );
        }
        assert!(output.stderr.is_empty());
    }

    fn execute(
        sources: &SourceMap,
        command: &str,
        body: &str,
        setting: &str,
        fault: Option<u32>,
        timeout: Duration,
    ) {
        let ir = entry_ir(sources, fault);
        let output = llvm_module::execute(
            &ir,
            "checked database application",
            64,
            timeout,
            &[command, body],
            Some(setting),
        );
        assert_output(output, fault.is_some());
    }

    fn sqlite(fixture: &Fixture<'_>) {
        execute(
            &fixture_sources(fixture),
            "store.item.exercise",
            fixture.body,
            SETTING,
            None,
            Duration::from_secs(20),
        );
    }

    #[test]
    fn database_fixtures_pass_the_existing_checker() {
        for fixture in [
            &CRUD,
            &BULK,
            &NULL_EQUALITY,
            &CURSOR,
            &SCALARS,
            &TRANSACTION,
            &STREAM,
            &SQL,
            &NUMERIC_ROLLBACK,
            &ENCODING_ROLLBACK,
            &HANDLER_TRANSACTION,
            &SEED,
            &EMPTY_BULK,
            &MIGRATION_INITIAL,
            &MIGRATION_UPGRADED,
            &CANCEL_STREAM,
        ] {
            checked(&fixture_sources(fixture));
        }
        let fixture = captured_errors();
        let app = format!("{ERRORS}{}", fixture.app);
        checked(&fixture_sources(&Fixture {
            app: &app,
            ..fixture
        }));
        let app = format!("{ERRORS}{}", TASK_ERROR.app);
        checked(&fixture_sources(&Fixture {
            app: &app,
            ..TASK_ERROR
        }));
        let app = format!("{ERRORS}{}", CAPTURED_NOT_FOUND.app);
        checked(&fixture_sources(&Fixture {
            app: &app,
            ..CAPTURED_NOT_FOUND
        }));
        checked(&fixture_sources_with_api(
            &CONSTRAINT_ROLLBACK,
            "cmd exercise = app.exercise\ncmd inspect = app.inspect",
        ));
        checked(&relations());
    }

    #[test]
    fn model_application_emits_objects_for_all_six_targets() {
        let sources = fixture_sources(&CRUD);
        let ir = dever_core::llvm::emit_application(&checked(&sources), &sources).unwrap();
        for target in Target::ALL {
            assert!(!emit_object(&ir, target).unwrap().is_empty());
        }
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_crud_defaults_owned_rows_and_repeat_entry() {
        sqlite(&CRUD);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_bulk_upsert_and_static_predicates() {
        sqlite(&BULK);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_nullable_equality_keeps_present_and_absent_binds() {
        sqlite(&NULL_EQUALITY);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_cursor_ties_nullable_next_and_offset_page() {
        sqlite(&CURSOR);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_persistent_scalars_choices_and_private_fields_round_trip() {
        sqlite(&SCALARS);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_nested_transactions_share_the_one_connection() {
        sqlite(&TRANSACTION);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_row_stream_full_early_stop_and_close_release_connections() {
        sqlite(&STREAM);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_static_sql_typed_optional_list_and_record_results() {
        sqlite(&SQL);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_captured_nested_database_faults_keep_exact_categories() {
        let fixture = captured_errors();
        let app = format!("{ERRORS}{}", fixture.app);
        sqlite(&Fixture {
            app: &app,
            ..fixture
        });
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_task_wait_keeps_database_failure_identity() {
        let app = format!("{ERRORS}{}", TASK_ERROR.app);
        sqlite(&Fixture {
            app: &app,
            ..TASK_ERROR
        });
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_numeric_fault_rolls_back_before_the_next_entry() {
        execute(
            &fixture_sources(&NUMERIC_ROLLBACK),
            "store.item.exercise",
            NUMERIC_ROLLBACK.body,
            SETTING,
            Some(2),
            Duration::from_secs(20),
        );
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_command_encoding_failure_rolls_back_before_retry() {
        execute(
            &fixture_sources(&ENCODING_ROLLBACK),
            "store.item.exercise",
            ENCODING_ROLLBACK.body,
            SETTING,
            Some(3),
            Duration::from_secs(20),
        );
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_uncaught_constraint_rolls_back_before_the_next_command() {
        constraint_sequence(SETTING, Duration::from_secs(20));
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_sequential_database_handler_inherits_transaction_context() {
        sqlite(&HANDLER_TRANSACTION);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_seed_history_is_reused_without_overwriting_business_rows() {
        sqlite(&SEED);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_empty_create_many_retains_valid_generated_defaults() {
        sqlite(&EMPTY_BULK);
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_related_cycles_join_batch_and_unloaded_values() {
        execute(
            &relations(),
            "store.account.exercise",
            "{}",
            SETTING,
            None,
            Duration::from_secs(20),
        );
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_schema_upgrade_preserves_rows_and_replays_migration_and_seed_once() {
        migration_sequence(SETTING, Duration::from_secs(20));
    }

    fn migration_sequence(setting: &str, timeout: Duration) {
        let directory = configured_directory(setting);
        for fixture in [&MIGRATION_INITIAL, &MIGRATION_UPGRADED] {
            let ir = entry_ir(&fixture_sources(fixture), None);
            assert_output(
                llvm_module::execute_in(
                    &directory,
                    &ir,
                    "successive checked Model revisions",
                    64,
                    timeout,
                    &["store.item.exercise", "{}"],
                ),
                false,
            );
        }
    }

    fn configured_directory(setting: &str) -> super::temp::TemporaryDirectory {
        let directory = super::temp::TemporaryDirectory::new();
        std::fs::create_dir(directory.path().join("config")).unwrap();
        std::fs::write(directory.path().join("config/setting.json"), setting).unwrap();
        directory
    }

    fn constraint_sequence(setting: &str, timeout: Duration) {
        let directory = configured_directory(setting);
        let sources = fixture_sources_with_api(
            &CONSTRAINT_ROLLBACK,
            "cmd exercise = app.exercise\ncmd inspect = app.inspect",
        );
        for (command, fault) in [
            ("store.item.exercise", Some(4)),
            ("store.item.inspect", None),
        ] {
            let ir = entry_ir(&sources, fault);
            assert_output(
                llvm_module::execute_in(
                    &directory,
                    &ir,
                    "constraint failure then persisted rollback inspection",
                    64,
                    timeout,
                    &[command, "{}"],
                ),
                fault.is_some(),
            );
        }
    }

    #[test]
    #[ignore = "requires explicitly prepared database runtime archive; owned SQLite only"]
    fn sqlite_cancelled_row_stream_drains_before_single_connection_fallback() {
        sqlite(&CANCEL_STREAM);
    }

    #[cfg(feature = "runtime-postgres")]
    fn postgres_case(sources: &SourceMap, command: &str, body: &str, fault: Option<u32>) {
        super::postgres::run_in_isolated_schema("llvm_database", |database| async move {
            execute(
                sources,
                command,
                body,
                &database.settings_json(),
                fault,
                Duration::from_secs(60),
            );
            Ok(())
        });
    }

    #[cfg(feature = "runtime-postgres")]
    #[test]
    #[ignore = "requires configured isolated database.postgres_test and an explicit database runtime archive"]
    fn postgres_checked_models_transactions_streams_and_schema_upgrade() {
        for fixture in [
            &CRUD,
            &BULK,
            &NULL_EQUALITY,
            &CURSOR,
            &SCALARS,
            &TRANSACTION,
            &STREAM,
            &SQL,
            &HANDLER_TRANSACTION,
            &SEED,
            &EMPTY_BULK,
            &CANCEL_STREAM,
        ] {
            postgres_case(
                &fixture_sources(fixture),
                "store.item.exercise",
                fixture.body,
                None,
            );
        }
        for fixture in [CAPTURED_NOT_FOUND, TASK_ERROR] {
            let app = format!("{ERRORS}{}", fixture.app);
            postgres_case(
                &fixture_sources(&Fixture {
                    app: &app,
                    ..fixture
                }),
                "store.item.exercise",
                fixture.body,
                None,
            );
        }
        postgres_case(
            &fixture_sources(&NUMERIC_ROLLBACK),
            "store.item.exercise",
            NUMERIC_ROLLBACK.body,
            Some(2),
        );
        postgres_case(&relations(), "store.account.exercise", "{}", None);
        super::postgres::run_in_isolated_schema("llvm_database", |database| async move {
            migration_sequence(&database.settings_json(), Duration::from_secs(60));
            Ok(())
        });
    }

    #[cfg(feature = "runtime-postgres")]
    #[test]
    #[ignore = "requires configured isolated database.postgres_test and an explicit database runtime archive"]
    fn postgres_command_encoding_failure_rolls_back_before_retry() {
        postgres_case(
            &fixture_sources(&ENCODING_ROLLBACK),
            "store.item.exercise",
            ENCODING_ROLLBACK.body,
            Some(3),
        );
    }

    #[cfg(feature = "runtime-postgres")]
    #[test]
    #[ignore = "requires configured isolated database.postgres_test and an explicit database runtime archive"]
    fn postgres_uncaught_constraint_rolls_back_before_the_next_command() {
        super::postgres::run_in_isolated_schema("llvm_database", |database| async move {
            constraint_sequence(&database.settings_json(), Duration::from_secs(60));
            Ok(())
        });
    }
}
