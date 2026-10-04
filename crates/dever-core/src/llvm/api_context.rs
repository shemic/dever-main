//! Request-only capabilities keep context and affine resources inside the runtime scope.
use super::*;

pub(super) const DECLARATIONS: &str = "\
%dever.api_cookie_options = type { ptr, ptr, i64, i8, i8, i64, i8 }
declare i32 @dever_rt_v1_api_request_id(ptr, ptr)
declare i32 @dever_rt_v1_api_method(ptr, ptr)
declare i32 @dever_rt_v1_api_path(ptr, ptr)
declare i32 @dever_rt_v1_api_client_address(ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_header(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_cookie(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_secret_cookie(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_response_header(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_response_cookie(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_response_secret_cookie(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_auth_site_key(ptr, ptr)
declare i32 @dever_rt_v1_auth_id(ptr, ptr)
declare i32 @dever_rt_v1_auth_session(ptr, ptr)
declare i32 @dever_rt_v1_auth_user_id(ptr, ptr, ptr)
declare i32 @dever_rt_v1_auth_tenant_id(ptr, ptr, ptr)
declare i32 @dever_rt_v1_auth_owns_user(i64, ptr, ptr)
declare i32 @dever_rt_v1_auth_issue(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_auth_issue_cookie(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_auth_clear_cookie(ptr, ptr)
declare i32 @dever_rt_v1_upload_filename(ptr, ptr, ptr)
declare i32 @dever_rt_v1_upload_content_type(ptr, ptr, ptr)
declare i32 @dever_rt_v1_upload_size(ptr, ptr, ptr)
declare i32 @dever_rt_v1_upload_close_take(ptr, ptr, ptr)
declare ptr @dever_rt_v1_upload_store_take(ptr, ptr)
declare ptr @dever_rt_v1_auth_permissions(ptr, ptr, ptr)
declare ptr @dever_rt_v1_auth_save_role(ptr, ptr, i8, ptr, ptr)
declare ptr @dever_rt_v1_auth_grant_role(i64, ptr, ptr)
declare ptr @dever_rt_v1_auth_revoke_role(i64, ptr, ptr)
declare ptr @dever_rt_v1_auth_disable_role(ptr, ptr)
";

pub(super) fn supported(operation: Intrinsic) -> bool {
    use Intrinsic::*;
    matches!(
        operation,
        ApiServe
            | ApiRequestId
            | ApiMethod
            | ApiPath
            | ApiClientAddress
            | ApiHeader
            | ApiCookie
            | ApiSecretCookie
            | ApiResponseHeader
            | ApiResponseCookie
            | ApiResponseSecretCookie
            | AuthIssue
            | AuthIssueCookie
            | AuthClearCookie
            | AuthOwnsUser
            | AuthId
            | AuthSession
            | AuthUserId
            | AuthTenantId
            | AuthPermissions
            | AuthSaveRole
            | AuthGrantRole
            | AuthRevokeRole
            | AuthDisableRole
            | SiteKey
            | UploadFilename
            | UploadContentType
            | UploadSize
            | UploadClose
            | UploadStore
    )
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn api_intrinsic(
        &mut self,
        operation: Intrinsic,
        arguments: &[Expression],
        values: &[String],
        result: &Expression,
    ) -> String {
        use Intrinsic::*;
        let mut args = arguments
            .iter()
            .zip(values)
            .map(|(argument, value)| format!("{} {value}", self.module.ty(&argument.ty)))
            .collect::<Vec<_>>();
        let symbol = match operation {
            ApiServe => {
                self.serve_api(result.span);
                return "zeroinitializer".into();
            }
            ApiRequestId => "api_request_id",
            ApiMethod => "api_method",
            ApiPath => "api_path",
            ApiClientAddress => "api_client_address",
            ApiHeader => "api_header",
            ApiCookie => "api_cookie",
            ApiSecretCookie => "api_secret_cookie",
            ApiResponseHeader => "api_response_header",
            ApiResponseCookie | ApiResponseSecretCookie => {
                let options = self.api_cookie_options(&arguments[2].ty, &values[2]);
                args[2] = format!("ptr {options}");
                if operation == ApiResponseCookie {
                    "api_response_cookie"
                } else {
                    "api_response_secret_cookie"
                }
            }
            AuthIssue | AuthIssueCookie => {
                let tenant = self.nullable_handle(&arguments[2].ty, &values[2]);
                args[2] = format!("ptr {tenant}");
                if operation == AuthIssue {
                    "auth_issue"
                } else {
                    "auth_issue_cookie"
                }
            }
            AuthClearCookie => "auth_clear_cookie",
            AuthOwnsUser => "auth_owns_user",
            AuthId => "auth_id",
            AuthSession => "auth_session",
            AuthUserId => "auth_user_id",
            AuthTenantId => "auth_tenant_id",
            SiteKey => "auth_site_key",
            UploadFilename => "upload_filename",
            UploadContentType => "upload_content_type",
            UploadSize => "upload_size",
            UploadClose | UploadStore => {
                let owned = self.temp();
                self.line(format!(
                    "{owned} = call ptr @dever_rt_v1_upload_retain(ptr {})",
                    values[0]
                ));
                args[0] = format!("ptr {owned}");
                if operation == UploadStore {
                    let operation = self.async_operation(
                        "upload_store_take",
                        vec![format!("ptr {owned}")],
                        result.span,
                    );
                    return self.async_resource_result(&operation, result, "Stored", None);
                }
                "upload_close_take"
            }
            AuthPermissions | AuthSaveRole | AuthGrantRole | AuthRevokeRole | AuthDisableRole => {
                return self.api_authorization_intrinsic(operation, arguments, values, result);
            }
            _ => unreachable!("checked request intrinsic"),
        };
        let value = self.runtime_call(
            &format!("dever_rt_v1_{symbol}"),
            args,
            &result.ty,
            matches!(result.ty, Type::Nullable(_)),
            matches!(result.ty, Type::Bool | Type::Unit).then_some("i8"),
            result.span,
        );
        if result.ty == Type::Unit {
            "zeroinitializer".into()
        } else {
            value
        }
    }

    fn api_authorization_intrinsic(
        &mut self,
        operation: Intrinsic,
        arguments: &[Expression],
        values: &[String],
        result: &Expression,
    ) -> String {
        use Intrinsic::*;
        let mut args = arguments
            .iter()
            .zip(values)
            .map(|(argument, value)| format!("{} {value}", self.module.ty(&argument.ty)))
            .collect::<Vec<_>>();
        let symbol = match operation {
            AuthPermissions => {
                let Type::List(element) = &result.ty else {
                    unreachable!("checked permission list");
                };
                let Type::Named(id) = element.as_ref() else {
                    unreachable!();
                };
                let callback = self.module.api_permission_pack(*id);
                args.extend([
                    format!("ptr @dever_type_{}", self.module.type_index(element)),
                    format!("ptr {callback}"),
                ]);
                "auth_permissions"
            }
            AuthSaveRole => {
                let flag = self.temp();
                self.line(format!("{flag} = zext i1 {} to i8", values[2]));
                args[2] = format!("i8 {flag}");
                "auth_save_role"
            }
            AuthGrantRole => "auth_grant_role",
            AuthRevokeRole => "auth_revoke_role",
            AuthDisableRole => "auth_disable_role",
            _ => unreachable!("authorization intrinsic"),
        };
        let operation = self.async_operation(symbol, args, result.span);
        self.await_operation(&operation, &result.ty, result.span, true)
    }

    pub(super) fn nullable_handle(&mut self, ty: &Type, value: &str) -> String {
        let present = self.temp();
        let handle = self.temp();
        let result = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {value}, 0",
            self.module.ty(ty)
        ));
        self.line(format!(
            "{handle} = extractvalue {} {value}, 1",
            self.module.ty(ty)
        ));
        self.line(format!(
            "{result} = select i1 {present}, ptr {handle}, ptr null"
        ));
        result
    }

    fn api_cookie_options(&mut self, ty: &Type, value: &str) -> String {
        let Type::Named(id) = ty else {
            unreachable!("checked CookieOptions");
        };
        let Shape::Record(fields) = self.module.program.types[*id].shape.clone() else {
            unreachable!();
        };
        let mut values = Vec::new();
        for (index, field) in fields.iter().enumerate() {
            let part = self.temp();
            self.line(format!("{part} = extractvalue %T{id} {value}, {index}"));
            values.push((field.ty.clone(), part));
        }
        let domain = self.nullable_handle(&values[1].0, &values[1].1);
        let Type::Named(same_site) = values[2].0 else {
            unreachable!();
        };
        let Shape::Choice(variants) = self.module.program.types[same_site].shape.clone() else {
            unreachable!();
        };
        let tag = self.temp();
        self.line(format!(
            "{tag} = extractvalue %T{same_site} {}, 0",
            values[2].1
        ));
        let mut same_site = "0".to_owned();
        for (name, kind) in [("Lax", 1), ("None", 2)] {
            let index = variants
                .iter()
                .position(|variant| variant.name == name)
                .unwrap();
            let matches = self.temp();
            let selected = self.temp();
            self.line(format!("{matches} = icmp eq i32 {tag}, {index}"));
            self.line(format!(
                "{selected} = select i1 {matches}, i64 {kind}, i64 {same_site}"
            ));
            same_site = selected;
        }
        let secure = self.temp();
        let http_only = self.temp();
        let max_present = self.temp();
        let max_age = self.temp();
        let max_flag = self.temp();
        self.line(format!("{secure} = zext i1 {} to i8", values[3].1));
        self.line(format!("{http_only} = zext i1 {} to i8", values[4].1));
        self.line(format!(
            "{max_present} = extractvalue {{ i1, i64 }} {}, 0",
            values[5].1
        ));
        self.line(format!(
            "{max_age} = extractvalue {{ i1, i64 }} {}, 1",
            values[5].1
        ));
        self.line(format!("{max_flag} = zext i1 {max_present} to i8"));
        let slot = self.entry_slot_ir("%dever.api_cookie_options");
        let mut row = "zeroinitializer".to_owned();
        for (index, field) in [
            format!("ptr {}", values[0].1),
            format!("ptr {domain}"),
            format!("i64 {same_site}"),
            format!("i8 {secure}"),
            format!("i8 {http_only}"),
            format!("i64 {max_age}"),
            format!("i8 {max_flag}"),
        ]
        .iter()
        .enumerate()
        {
            let next = self.temp();
            self.line(format!(
                "{next} = insertvalue %dever.api_cookie_options {row}, {field}, {index}"
            ));
            row = next;
        }
        self.line(format!("store %dever.api_cookie_options {row}, ptr {slot}"));
        slot
    }
}

impl Module<'_> {
    fn api_permission_pack(&mut self, id: usize) -> String {
        let name = format!("@dever_permission_pack_{id}");
        // The compiler-owned callback is reused by every call of permissions().
        if self.api_permission_types.insert(id) {
            let mut body = format!(
                "define internal void {name}(ptr %key, ptr %component, ptr %domain, ptr %site, ptr %action, ptr %method, ptr %out) {{\nentry:\n"
            );
            for (index, field) in ["key", "component", "domain", "site", "action", "method"]
                .iter()
                .enumerate()
            {
                writeln!(body, "  %owned{index} = call ptr @dever_rt_v1_text_retain(ptr %{field})\n  %field{index} = getelementptr %T{id}, ptr %out, i32 0, i32 {index}\n  store ptr %owned{index}, ptr %field{index}").unwrap();
            }
            body.push_str("  ret void\n}\n");
            self.declarations.push_str(&body);
        }
        name
    }
}
