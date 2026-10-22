use super::rule;
use crate::patch::{Action, Rule};

pub fn hmcl(remove_updates: bool) -> Vec<Rule> {
    let mut rules = vec![
        rule(
            "org/jackhuang/hmcl/util/RegionUtils",
            "checkMachineIsChinaMainland",
            Some("()Z"),
            Action::ReturnFalse,
            "地区判定：初始化时强制判定为非中国大陆",
        ),
        rule(
            "org/jackhuang/hmcl/util/RegionUtils",
            "isIsChinaMainlandMachine",
            Some("()Z"),
            Action::ReturnFalse,
            "地区判定：机器地区查询返回 false",
        ),
        rule(
            "org/jackhuang/hmcl/util/RegionUtils",
            "isIsChinaMainlandNetwork",
            Some("()Z"),
            Action::ReturnFalse,
            "地区判定：网络地区查询返回 false，恢复下载页",
        ),
        rule(
            "org/jackhuang/hmcl/upgrade/IntegrityChecker",
            "isSelfVerified",
            Some("()Z"),
            Action::ReturnTrue,
            "自校验：始终视为官方完整构建，避免修改后被拒绝",
        ),
    ];
    if remove_updates {
        rules.extend([
            rule(
                "org/jackhuang/hmcl/upgrade/UpdateChecker",
                "init",
                Some("()V"),
                Action::ReturnVoid,
                "启动时不再发起更新检查",
            ),
            rule(
                "org/jackhuang/hmcl/upgrade/UpdateChecker",
                "requestCheckUpdate",
                None,
                Action::ReturnVoid,
                "手动/自动更新检查直接返回",
            ),
            rule(
                "org/jackhuang/hmcl/upgrade/UpdateChecker",
                "isOutdated",
                Some("()Z"),
                Action::ReturnFalse,
                "更新状态永远为“已是最新”",
            ),
            rule(
                "org/jackhuang/hmcl/upgrade/UpdateChecker",
                "isCheckingUpdate",
                Some("()Z"),
                Action::ReturnFalse,
                "隐藏“正在检查更新”状态",
            ),
        ]);
    }
    rules
}
