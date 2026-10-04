# CMS 业务闭环实施记录

## 2026-10-04 基线
用户批准本轮实施。旧勾选不代表当前业务验收完成：每套45个生产文件/12领域，3个应用测试中2个固定字符串断言；cms_acceptance只比较API/Model。定时发布用了enqueue而不是enqueue_at，修订版本用count+1。此前真实PG/RBAC/租户和核心Job验收有效，但不代替CMS业务用例。

## 执行顺序
- [x] 更新要求/设计/上下文，保存改前备份。
- [x] 分类/媒体关联、文章版本竞争、修订事务。
- [x] 到期入队、共享发布和任务幂等。
- [x] 替换空测试、同步Markdown、完善等价与CLI预期。
- [x] 有界HTTP验收：身份权限、编辑冲突、上传、到期/重试/回滚。
- [x] 两套check/fmt/test、相关Rust/Python检查和真实验收。
- [x] 独立复核，修复阻塞，更新示例与记录。

## 验证约束
磁盘约462MiB空闲，复用target/debug/dever和runtime pack，Cargo单并发。自有HTTP/Worker前说明；不连真实数据库，不运行全量测试/压测，不变更全局命令/服务。配置只在自有临时项目写config/setting.json；子进程有超时与清理，分别记录通过/失败/未运行。

## 结果
本轮业务闭环已完成。真实HTTP、CLI、核心约束回归、独立复核和静态质量门均通过；未执行范围外的全量CI、真实PG或压测，不将当前结果当作这些验证的替代。所有改动保留在工作区，未创建Git提交。

## 已完成的定向证据与直接阻塞
- 两套源改为46个生产文件+5个真实应用测试；51对源码声明一致，check/fmt及各5/5应用测试已通过。源测试验证正常业务输出，拒绝和失效放在真实HTTP验证，不伪造认证上下文。
- Python CMS调用迁移单测12/12；SQL View/字段范围/跨域约束4/4；API声明与GET只读SQL规则18/18；CMS语义/配置/测试等价2/2通过。
- HTTP第1轮在build发现published_detail误删既有业务校验，已恢复；第2轮构建成功，但bootstrap启动拒绝global Session读取与tenant Article写入同一事务。日志保存在target/cms-business-acceptance-{1,2}/，尚未进入HTTP业务断言。
- 直接语言修复：SQL可返回同领域App View，普通SQL结果record拒绝未经数据库解码证明的bounds；仅完整自有表简单投影可证明只读，两方言都通过才允许GET，其他SQL保留write效果。复用现有类型/行解码/事务，不增依赖和运行时ABI。
- 已修可信身份的名义ModelId取值，以已验证的auth metadata替代业务重复读取global Session；不放宽跨库事务，不将模型ID改成Int。
- 第3轮真实HTTP/Worker验收10组全部通过，report在target/cms-business-acceptance-3/report.json：真实登录/角色拒绝、上传清理、关联/非作者/过期版本、修订晚失败回滚、两请求单胜者、发布快照与晚失败回滚、未来到期/同刻任务、完成任务重放、失败重试、撤权阻断任务、会话注销/过期与站点隔离。fixture程序8,465,384 bytes，临时配置/DB和进程已清理；非压测。
- 独立审查确证REST字段create/replace也可产生typed身份getter，原新增validator只遍历显式API；现已用同一IdentityContext校验API、REST create/replace/search与名义ID owner。新增正反例、跨站点provider错配、native/LLVM生成验证。完整API目标26/26通过，原错误fixture被CLI明确C006拒绝后已清理。普通Int和nullable边界保留。
- 最终源码等价2/2、typed_sql 4/4、Python CMS调用单测12/12。CLI精确用例2/2：双格式各5个应用测试各运行两次（故意阻断部署DB，证明测试隔离），check/run/build及无源码打包运行；混合6用例验证仅数据库用例初始化临时DB。更新了该混合用例残留的旧3测试预期。
- HTTP最终运行器显式关闭SQLite连接；复用已构建程序复验10组全部通过，最终报告target/cms-business-acceptance-final/report.json。不改源码/部署配置，不遗留HTTP/Worker/测试进程。此结果不覆盖真实PG、浏览器HTTPS或性能/长压测。
- 两套最终check/fmt通过，check各有7条W001未读取返回值提示，非错误；语言只允许零输出调用作为动作语句，未用虚假读取或抑制消除提示。Rust改动文件rustfmt --check通过。
- 定向clippy通过（dever-core/dever-tests/dever-cli，lib、dever bin及api_declarations/orm_native_sql/contract_execution/cms_acceptance/cms_project，sqlite feature，-D warnings）。最终diff检查通过。独立复核指出的SQL bounds和REST身份绑定缺口均修复并保留回归；主线程确认实际测试输出。任务直接在既有main工作区完成、没有PR分支，归档使用skip-branch-validation且no-commit。
- 磁盘紧张时暂移的可再生成target/native-artifacts已压缩为target/cms-workflows-before.v1FzWO/native-artifacts-cache.tar.xz（约190MiB）。先逐文件比较，再验证重压缩前后tar SHA256一致（675338b0d4af58b76689ae3ec9bc2c7202a1b5aa3e3f117d0df5930f7e88a7d0）；原路径恢复普通空缓存目录，软链和自有内存盘临时副本均已清理。源码、数据库和其他服务未动。

## Bug Analysis: SQL投影与业务集成边界
1. 根因：跨层合同与测试覆盖缺口。私有Model不能穿越App边界，但SQL结果又只能是Model内部record；查询不能直接返回已定义的App View。数据库decoder也不会证明源级bounds。
2. 失败尝试：只开放同域私有record引用仍会触发公开App签名约束；把Session重新读进文章调用链则实际触发跨库事务限制。不能通过放宽隐私或事务检查解决。
3. 防止复发：共享SQL result检查同时验证归属、字段类型/可见性和bounds；GET效果采用完整受限语法证明，保留反例；HTTP按正常入口启动、验证完整提交与回滚。
4. 扩展核对：双源码、旧CMS性能/PG调用方及CLI测试预期同步迁移；原SQL私有record同样拒绝未证明bounds。
5. 已更新backend/database-guidelines.md和LANGUAGE.md；本仓库无对应spec模板副本，不创建；按用户约束不提交Git。
