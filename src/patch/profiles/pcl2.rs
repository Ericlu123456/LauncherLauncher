use super::{pcl2ce, rule};
use crate::patch::{Action, Rule};

pub fn pcl2(remove_updates: bool) -> Vec<Rule> {
    // PCL2 and PCL2-CE share the same PCL.* root namespace in recent builds.
    // Apply the region rules as well; absent classes are simply not matched.
    let mut rules = pcl2ce(false);
    rules.push(rule(
        "PCL.PageDownloadInstall",
        "LoaderInit",
        None,
        Action::Pcl2UnlockDownload,
        "恢复下载页的真实版本列表加载器（绕过 2.99 的“关闭下载”分支）",
    ));
    if remove_updates {
        rules.extend([
            rule(
                "PCL.ModSecret",
                "UpdateCheckByButton",
                None,
                Action::ReturnVoid,
                "“检查更新”按钮不再发起请求",
            ),
            rule(
                "PCL.ModSecret",
                "UpdateStart",
                None,
                Action::ReturnVoid,
                "服务器公告里的“更新 PCL”按钮不再下载新版本",
            ),
            rule(
                "PCL.ModUpdate",
                "UpdateCheck",
                None,
                Action::ReturnVoid,
                "旧版更新入口（若存在）直接返回",
            ),
        ]);
    }
    rules
}
