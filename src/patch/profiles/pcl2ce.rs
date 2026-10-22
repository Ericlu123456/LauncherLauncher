use super::rule;
use crate::patch::{Action, Rule};

pub fn pcl2ce(remove_updates: bool) -> Vec<Rule> {
    let mut rules = vec![
        rule(
            "PCL.Core.App.Region.RegionAssessment",
            "get_IsRestricted",
            None,
            Action::ReturnFalse,
            "地区评估：IsRestricted 永远为 false",
        ),
        rule(
            "PCL.RestrictedFeaturePolicy",
            "get_IsRestricted",
            None,
            Action::ReturnFalse,
            "全局限制标志：IsRestricted 永远为 false",
        ),
        rule(
            "PCL.RestrictedFeaturePolicy",
            "Check",
            None,
            Action::ReturnTrue,
            "功能检查：Check(...) 永远通过",
        ),
        rule(
            "PCL.RestrictedFeaturePolicy",
            "ThrowIfRestricted",
            None,
            Action::ReturnVoid,
            "功能检查：ThrowIfRestricted(...) 不再抛异常",
        ),
    ];
    if remove_updates {
        rules.extend([
            rule(
                "PCL.UpdateManager",
                "UpdateStart",
                None,
                Action::ReturnVoid,
                "禁用 PCL2-CE 的更新下载/二进制下发",
            ),
            rule(
                "PCL.PageSetupUpdate",
                "CheckUpdate",
                None,
                Action::ReturnVoid,
                "设置页的更新检查直接返回",
            ),
            rule(
                "PCL.UpdatesWrapperModel",
                "IsLatest",
                None,
                Action::ReturnTrue,
                "所有版本源都报告“已是最新”",
            ),
            rule(
                "PCL.UpdatesWrapperModel",
                "IsLatestAsync",
                None,
                Action::ReturnVoid,
                "异步更新检查直接返回（若存在）",
            ),
        ]);
    }
    rules
}
