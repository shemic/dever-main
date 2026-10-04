use dever_runtime::auth::store::{self, Permission, Role};
use dever_runtime::{database, orm};

static PERMISSIONS: &[Permission] = &[
    Permission {
        key: "news.article.admin.publish",
        component: "news",
        domain: "article",
        site: "admin",
        action: "publish",
        method: "POST",
    },
    Permission {
        key: "news.article.admin.delete",
        component: "news",
        domain: "article",
        site: "admin",
        action: "delete",
        method: "DELETE",
    },
    Permission {
        key: "news.article.front.read",
        component: "news",
        domain: "article",
        site: "front",
        action: "read",
        method: "GET",
    },
];

pub async fn assert_contract(catalog: database::Database, authorization: database::Database) {
    store::sync_catalog(catalog.clone(), PERMISSIONS)
        .await
        .unwrap();
    create_legacy_role_schema(&authorization).await;
    store::initialize_roles(authorization.clone())
        .await
        .unwrap();

    assert!(
        store::authorize(
            authorization.clone(),
            9,
            "admin",
            "news.article.admin.publish",
        )
        .await
        .unwrap()
    );
    let version = authorization
        .query(
            database::Sql {
                sqlite: "SELECT version FROM _dever_auth_version",
                postgres: "SELECT version FROM _dever_auth_version",
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(version[0].get(0).unwrap(), &orm::Value::Int(2));

    let listed = store::list_permissions(catalog.clone()).await.unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].key, "news.article.admin.delete");
    assert_eq!(listed[0].method, "DELETE");
    assert_eq!(listed[2].key, "news.article.front.read");

    let publisher = Role {
        id: "publisher".into(),
        site: "admin".into(),
        name: "Publisher".into(),
        all_permissions: false,
    };
    let deleter = Role {
        id: "deleter".into(),
        site: "admin".into(),
        name: "Deleter".into(),
        all_permissions: false,
    };
    let reader = Role {
        id: "publisher".into(),
        site: "front".into(),
        name: "Reader".into(),
        all_permissions: false,
    };
    save_role(
        &catalog,
        &authorization,
        &publisher,
        "news.article.admin.publish",
    )
    .await;
    save_role(
        &catalog,
        &authorization,
        &deleter,
        "news.article.admin.delete",
    )
    .await;
    save_role(&catalog, &authorization, &reader, "news.article.front.read").await;
    assert!(
        store::grant_role(authorization.clone(), 4, "front", "deleter")
            .await
            .is_err()
    );

    assert!(
        !store::authorize(
            authorization.clone(),
            1,
            "admin",
            "news.article.admin.publish",
        )
        .await
        .unwrap()
    );
    store::grant_role(authorization.clone(), 1, "admin", "publisher")
        .await
        .unwrap();
    assert!(
        !store::authorize(authorization.clone(), 1, "front", "news.article.front.read",)
            .await
            .unwrap()
    );
    store::grant_role(authorization.clone(), 1, "front", "publisher")
        .await
        .unwrap();
    assert!(
        store::authorize(
            authorization.clone(),
            1,
            "admin",
            "news.article.admin.publish",
        )
        .await
        .unwrap()
    );
    assert!(
        !store::authorize(
            authorization.clone(),
            1,
            "admin",
            "news.article.admin.delete",
        )
        .await
        .unwrap()
    );
    assert!(
        store::authorize(authorization.clone(), 1, "front", "news.article.front.read",)
            .await
            .unwrap()
    );

    store::grant_role(authorization.clone(), 2, "admin", "publisher")
        .await
        .unwrap();
    store::grant_role(authorization.clone(), 2, "admin", "deleter")
        .await
        .unwrap();
    for permission in ["news.article.admin.publish", "news.article.admin.delete"] {
        assert!(
            store::authorize(authorization.clone(), 2, "admin", permission)
                .await
                .unwrap()
        );
    }

    store::disable_role(authorization.clone(), "admin", "deleter")
        .await
        .unwrap();
    assert!(
        store::authorize(
            authorization.clone(),
            2,
            "admin",
            "news.article.admin.publish",
        )
        .await
        .unwrap()
    );
    assert!(
        !store::authorize(
            authorization.clone(),
            2,
            "admin",
            "news.article.admin.delete",
        )
        .await
        .unwrap()
    );

    let owner_role = store::provision_owner(authorization.clone(), 3, "admin")
        .await
        .unwrap();
    assert_eq!(owner_role, "_dever_owner:admin");
    let reserved = Role {
        id: owner_role.clone(),
        site: "admin".into(),
        name: "Not Owner".into(),
        all_permissions: true,
    };
    assert!(
        store::save_role(catalog.clone(), authorization.clone(), &reserved, &[])
            .await
            .is_err()
    );
    assert!(
        store::grant_role(authorization.clone(), 4, "admin", &owner_role)
            .await
            .is_err()
    );
    assert!(
        store::revoke_role(authorization.clone(), 3, "admin", &owner_role)
            .await
            .is_err()
    );
    assert!(
        store::disable_role(authorization.clone(), "admin", &owner_role)
            .await
            .is_err()
    );
    assert!(
        store::authorize(
            authorization.clone(),
            3,
            "admin",
            "news.article.admin.publish",
        )
        .await
        .unwrap()
    );
    assert!(
        store::authorize(
            authorization.clone(),
            3,
            "admin",
            "news.article.admin.delete",
        )
        .await
        .unwrap()
    );
    assert!(
        !store::authorize(authorization.clone(), 3, "front", "news.article.front.read",)
            .await
            .unwrap()
    );

    store::revoke_role(authorization.clone(), 1, "admin", "publisher")
        .await
        .unwrap();
    assert!(
        !store::authorize(
            authorization.clone(),
            1,
            "admin",
            "news.article.admin.publish",
        )
        .await
        .unwrap()
    );
    assert!(
        store::authorize(authorization.clone(), 1, "front", "news.article.front.read",)
            .await
            .unwrap()
    );
    assert!(
        store::authorize(catalog.clone(), 1, "admin", "news.article.front.read",)
            .await
            .is_err()
    );

    store::sync_catalog(catalog.clone(), &[PERMISSIONS[0], PERMISSIONS[2]])
        .await
        .unwrap();
    let listed = store::list_permissions(catalog.clone()).await.unwrap();
    assert_eq!(listed.len(), 2);
    let inactive = catalog
        .query(
            database::Sql {
                sqlite: "SELECT active FROM _dever_permission WHERE key='news.article.admin.delete'",
                postgres: "SELECT active FROM _dever_permission WHERE key='news.article.admin.delete'",
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(inactive[0].get(0).unwrap(), &orm::Value::Int(0));
    assert!(
        store::save_role(
            catalog,
            authorization,
            &deleter,
            &["news.article.admin.delete".into()],
        )
        .await
        .is_err()
    );
}

async fn create_legacy_role_schema(database: &database::Database) {
    for sql in [
        "CREATE TABLE _dever_auth_role (id TEXT PRIMARY KEY, site TEXT NOT NULL, name TEXT NOT NULL, all_permissions BIGINT NOT NULL CHECK(all_permissions IN (0,1)), enabled BIGINT NOT NULL CHECK(enabled IN (0,1)))",
        "CREATE TABLE _dever_auth_role_permission (role_id TEXT NOT NULL REFERENCES _dever_auth_role(id) ON DELETE CASCADE, permission_key TEXT NOT NULL, PRIMARY KEY(role_id,permission_key))",
        "CREATE TABLE _dever_auth_user_role (user_id BIGINT NOT NULL, role_id TEXT NOT NULL REFERENCES _dever_auth_role(id) ON DELETE CASCADE, PRIMARY KEY(user_id,role_id))",
        "CREATE INDEX _dever_auth_role_site ON _dever_auth_role(site,enabled)",
        "CREATE INDEX _dever_auth_user_role_user ON _dever_auth_user_role(user_id,role_id)",
        "INSERT INTO _dever_auth_role(id,site,name,all_permissions,enabled) VALUES ('publisher','admin','Legacy Publisher',0,1)",
        "INSERT INTO _dever_auth_role_permission(role_id,permission_key) VALUES ('publisher','news.article.admin.publish')",
        "INSERT INTO _dever_auth_user_role(user_id,role_id) VALUES (9,'publisher')",
    ] {
        database
            .execute(
                database::Sql {
                    sqlite: sql,
                    postgres: sql,
                },
                vec![],
            )
            .await
            .unwrap();
    }
}

async fn save_role(
    catalog: &database::Database,
    authorization: &database::Database,
    role: &Role,
    permission: &str,
) {
    store::save_role(
        catalog.clone(),
        authorization.clone(),
        role,
        &[permission.into()],
    )
    .await
    .unwrap();
}
