//! 账号选择策略

use serde::{Deserialize, Serialize};

/// 选择策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionStrategy {
    /// 轮询
    #[default]
    RoundRobin,
    /// 随机
    Random,
    /// 最少使用
    LeastUsed,
    /// 依次使用，当前账号耗尽后再切到下一个
    SequentialExhaust,
}

impl SelectionStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RoundRobin => "round-robin",
            Self::Random => "random",
            Self::LeastUsed => "least-used",
            Self::SequentialExhaust => "sequential-exhaust",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SelectionStrategy;

    #[test]
    fn test_sequential_exhaust_as_str() {
        assert_eq!(
            SelectionStrategy::SequentialExhaust.as_str(),
            "sequential-exhaust"
        );
    }
}
