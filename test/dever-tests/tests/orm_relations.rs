use std::ffi::OsString;
use std::fs;
use std::process::Command;

use dever_core::source::SourceMap;

const ORGANIZATION: &str = r#"
type Organization {
  name: Text(1, 64)
}

relation users = app.people.model.user.organization_id
"#;

const USER: &str = r#"
type User {
  name: Text(1, 64)
  organization_id: app.people.model.organization.id?
  manager_id: app.people.model.user.id?
}

relation posts = app.people.model.post.author_id
relation other_posts = app.people.model.post.author_id
relation unloaded_posts = app.people.model.post.author_id
"#;

const POST: &str = r#"
type Post {
  author_id: app.people.model.user.id
  title: Text(1, 64)
}
"#;

const APP: &str = r#"
organization_name(value: Related.Unloaded<app.people.model.organization.Organization?>()) (name: Text) {
  name = "unloaded"
}

organization_name(value: Related.Loaded<app.people.model.organization.Organization?>(organization)) (name: Text) {
  name = nullable_organization_name(organization)
}

nullable_organization_name(organization: null) (name: Text) {
  name = "none"
}

nullable_organization_name(organization: app.people.model.organization.Organization) (name: Text) {
  name = organization.name
}

manager_name(value: Related.Unloaded<app.people.model.user.User?>()) (name: Text) {
  name = "unloaded"
}

manager_name(value: Related.Loaded<app.people.model.user.User?>(manager)) (name: Text) {
  name = nullable_manager_name(manager)
}

nullable_manager_name(manager: null) (name: Text) {
  name = "none"
}

nullable_manager_name(manager: app.people.model.user.User) (name: Text) {
  name = manager.name
}

post_count(value: Related.Unloaded<List<app.people.model.post.Post>>()) (count: Int) {
  count = -1
}

post_count(value: Related.Loaded<List<app.people.model.post.Post>>(posts)) (count: Int) {
  count = length(posts)
}

show(user: app.people.model.user.User) () {
  blocking(dever.io.println(organization_name(user.organization)))
  blocking(dever.io.println(manager_name(user.manager)))
  blocking(dever.io.println(int.to_text(post_count(user.posts))))
  blocking(dever.io.println(int.to_text(post_count(user.other_posts))))
  blocking(dever.io.println(int.to_text(post_count(user.unloaded_posts))))
}

users() (page: app.people.model.user.Page) {
  page = model.user.list({
    order = id.asc
    size = 10
    with = [organization, manager, posts, other_posts]
  })
}

transaction transaction_users() (page: app.people.model.user.Page) {
  page = users()
}

related_first() (item: app.people.model.user.User?) {
  item = model.user.first({
    order = id.asc
    with = [organization, manager, posts]
  })
}

related_cursor() (page: app.people.model.user.Cursor) {
  page = model.user.cursor({
    order = id.asc
    size = 10
    with = [organization, manager, posts]
  })
}

main() () {
  organization = model.organization.create({ name = "Acme" })
  first_user = model.user.create({
    name = "First"
    organization_id = organization.id
  })
  second_user = model.user.create({
    name = "Second"
    manager_id = first_user.id
  })
  first_post = model.post.create({
    author_id = first_user.id
    title = "One"
  })
  second_post = model.post.create({
    author_id = first_user.id
    title = "Two"
  })

  unloaded = model.user.get(first_user.id)
  blocking(dever.io.println(organization_name(unloaded.organization)))

  page = transaction_users()
  each(show, page.items)
}
"#;

fn sources(app: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("app/people/model/organization.dever", ORGANIZATION);
    sources.add("app/people/model/user.dever", USER);
    sources.add("app/people/model/post.dever", POST);
    sources.add("app/people/app.dever", app);
    sources
}

fn checked(app: &str) -> (SourceMap, dever_core::hir::Program) {
    let sources = sources(app);
    let program = dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    (sources, program)
}

fn errors(app: &str) -> String {
    let sources = sources(app);
    dever_core::check(&sources)
        .unwrap_err()
        .iter()
        .map(|error| error.render(&sources))
        .collect()
}

#[test]
fn logical_cross_scope_relations_cannot_be_loaded_implicitly() {
    let mut sources = SourceMap::default();
    sources.add(
        "platform/tenant/model.dever",
        "global type Tenant { name: Text }\n",
    );
    sources.add(
        "news/article/model.dever",
        "type Article { tenant_id: platform.tenant.model.id\n title: Text }\n",
    );
    sources.add(
        "news/article/app.dever",
        "list() (page: model.Page) { page = model.list({ with = [tenant] }) }\n",
    );
    let errors = dever_core::check(&sources).unwrap_err();
    let rendered = errors
        .iter()
        .map(|error| error.render(&sources))
        .collect::<String>();
    assert!(
        rendered.contains("logical cross-scope relation 'tenant' cannot be loaded"),
        "{rendered}",
    );
}

fn configure(native: &dever_core::native::NativeProgram) {
    let root = native.executable().parent().unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/setting.json"),
        r#"{
  "database": {
    "default": {
      "type": "sqlite",
      "path": "data/db/app.db",
      "max_connections": 1,
      "max_page_size": 10
    }
  }
}"#,
    )
    .unwrap();
}

#[test]
fn with_is_static_and_generates_join_and_one_batched_to_many_shape() {
    let source_map = sources(APP);
    let formatted = dever_core::format::format(&source_map.files()[3]).unwrap();
    assert!(formatted.contains("Related.Loaded<app.people.model.user.User?>"));
    checked(&formatted);

    let (sources, program) = checked(APP);
    let snapshot = program.model_snapshot();
    assert!(!snapshot.contains("field organization "), "{snapshot}");
    assert!(!snapshot.contains("field posts "), "{snapshot}");
    let api = program.api_snapshot();
    assert!(
        api.contains("organization: Related<app.people.model.organization.Organization?>"),
        "{api}"
    );
    assert!(
        api.contains("posts: Related<List<app.people.model.post.Post>>"),
        "{api}"
    );

    let native = dever_core::native::emit(&program, &sources, "app.people.users").unwrap();
    assert!(
        native.contains("LEFT JOIN \\\"organization\\\""),
        "{native}"
    );
    assert!(
        native.contains("LEFT JOIN \\\"user\\\" AS \\\"_dever_relation_1\\\""),
        "{native}"
    );
    assert!(
        native
            .contains("ROW_NUMBER() OVER (PARTITION BY \\\"author_id\\\" ORDER BY \\\"id\\\" ASC)"),
        "{native}"
    );
    assert!(native.contains(".chunks(998)"), "{native}");
    assert_eq!(
        native.matches("database.query_owned(").count(),
        2,
        "{native}"
    );
    assert!(
        native.contains(
            "sqlite: \"SELECT COUNT(*) FROM \\\"user\\\"\", postgres: \"SELECT COUNT(*) FROM \\\"user\\\"\""
        ),
        "{native}"
    );

    let first = dever_core::native::emit(&program, &sources, "app.people.related_first").unwrap();
    assert!(first.contains("LEFT JOIN \\\"organization\\\""), "{first}");
    assert_eq!(first.matches("database.query_owned(").count(), 1, "{first}");

    let cursor = dever_core::native::emit(&program, &sources, "app.people.related_cursor").unwrap();
    assert!(
        cursor.contains("LEFT JOIN \\\"organization\\\""),
        "{cursor}"
    );
    assert_eq!(
        cursor.matches("database.query_owned(").count(),
        1,
        "{cursor}"
    );
    assert!(!cursor.contains("COUNT(*)"), "{cursor}");
}

#[test]
fn relation_batch_sql_reserves_one_parameter_for_the_per_parent_limit() {
    let sql = dever_runtime::database::relation_sql(
        "SELECT * FROM child WHERE parent_id IN (",
        ") AND rank <= ",
        " ORDER BY parent_id, id",
        998,
    );
    assert_eq!(sql.sqlite.matches('?').count(), 999, "{}", sql.sqlite);
    assert!(
        sql.sqlite.contains("?998) AND rank <= ?999"),
        "{}",
        sql.sqlite
    );
    assert_eq!(sql.postgres.matches('$').count(), 999, "{}", sql.postgres);
    assert!(
        sql.postgres.contains("$998) AND rank <= $999"),
        "{}",
        sql.postgres
    );
}

#[test]
fn with_rejects_unknown_duplicate_and_stream_relations() {
    let unknown = APP.replace("with = [organization, manager, posts]", "with = [missing]");
    assert!(errors(&unknown).contains("unknown Model relation 'missing'"));

    let duplicate = APP.replace(
        "with = [organization, manager, posts]",
        "with = [organization, organization]",
    );
    assert!(errors(&duplicate).contains("with cannot repeat relation 'organization'"));

    let stream = APP.replace("page = model.user.list({", "page = model.user.stream({");
    assert!(errors(&stream).contains("unknown query option 'with'"));
}

#[test]
fn related_patterns_are_typed_exhaustive_and_do_not_relax_regular_cycles() {
    let invalid_payload = APP.replace(
        "main() () {",
        "invalid(value: Related<Int>) () {\n}\n\nmain() () {",
    );
    assert!(
        errors(&invalid_payload)
            .contains("Related payload must be a Model, nullable Model or List<Model>")
    );
    let nullable_list = APP.replace(
        "main() () {",
        "invalid(value: Related<List<app.people.model.user.User>?>) () {\n}\n\nmain() () {",
    );
    assert!(
        errors(&nullable_list)
            .contains("Related payload must be a Model, nullable Model or List<Model>")
    );

    let missing_unloaded = APP.replace(
        concat!(
            "organization_name(value: Related.Unloaded<",
            "app.people.model.organization.Organization?>()) (name: Text) {\n",
            "  name = \"unloaded\"\n}\n\n",
        ),
        "",
    );
    assert!(errors(&missing_unloaded).contains("do not cover every input combination"));

    let recursive = r#"
type Node {
  child: Node?
}
"#;
    assert!(errors(recursive).contains("recursive type definitions are not allowed"));
}

#[test]
fn sqlite_native_loads_requested_relations_and_exposes_related_matching() {
    let (sources, program) = checked(APP);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let native = dever_core::native::compile_application(
        &program,
        &sources,
        "app.people.main",
        &rustc,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    configure(&native);
    let output = Command::new(native.executable()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "unloaded\nAcme\nnone\n2\n2\n-1\nnone\nFirst\n0\n0\n-1\n"
    );
    assert!(output.stderr.is_empty());
}
