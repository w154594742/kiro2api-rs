//! 账号状态管理

use crate::kiro::model::credentials::KiroCredentials;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 账号状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    /// 可用
    Active,
    /// 冷却中（限流）
    Cooldown,
    /// 配额耗尽（等待额度恢复）
    Exhausted,
    /// 已失效
    Invalid,
    /// 已禁用
    Disabled,
}

/// 账号信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// 唯一标识
    pub id: String,
    /// 显示名称
    pub name: String,
    /// 凭证信息
    #[serde(skip_serializing)]
    pub credentials: KiroCredentials,
    /// 状态
    pub status: AccountStatus,
    /// 请求计数
    pub request_count: u64,
    /// 失败计数
    pub error_count: u64,
    /// 最后使用时间
    pub last_used_at: Option<DateTime<Utc>>,
    /// 冷却结束时间
    pub cooldown_until: Option<DateTime<Utc>>,
    /// 配额耗尽恢复时间
    pub exhausted_until: Option<DateTime<Utc>>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
}

impl Account {
    /// 创建新账号
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        credentials: KiroCredentials,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            credentials,
            status: AccountStatus::Active,
            request_count: 0,
            error_count: 0,
            last_used_at: None,
            cooldown_until: None,
            exhausted_until: None,
            created_at: Utc::now(),
        }
    }

    /// 检查是否可用
    pub fn is_available(&self) -> bool {
        match self.status {
            AccountStatus::Active => true,
            AccountStatus::Cooldown => {
                // 检查冷却是否结束
                self.cooldown_until
                    .map(|until| Utc::now() >= until)
                    .unwrap_or(true)
            }
            AccountStatus::Exhausted => self
                .exhausted_until
                .map(|until| Utc::now() >= until)
                .unwrap_or(false),
            _ => false,
        }
    }

    /// 记录使用
    pub fn record_use(&mut self) {
        self.request_count += 1;
        self.last_used_at = Some(Utc::now());
        // 如果冷却结束，恢复为活跃状态
        if self.status == AccountStatus::Cooldown && self.is_available() {
            self.status = AccountStatus::Active;
            self.cooldown_until = None;
        }
        if self.status == AccountStatus::Exhausted && self.is_available() {
            self.status = AccountStatus::Active;
            self.exhausted_until = None;
        }
    }

    /// 记录错误
    pub fn record_error(&mut self, is_rate_limit: bool) {
        self.error_count += 1;
        if is_rate_limit {
            // 限流，进入冷却
            self.status = AccountStatus::Cooldown;
            self.cooldown_until = Some(Utc::now() + chrono::Duration::minutes(5));
        }
    }

    /// 标记为失效（自动转为禁用）
    pub fn mark_invalid(&mut self) {
        self.status = AccountStatus::Disabled;
        self.cooldown_until = None;
        self.exhausted_until = None;
    }

    /// 标记为配额耗尽
    pub fn mark_exhausted(&mut self, next_reset: Option<DateTime<Utc>>) {
        self.status = AccountStatus::Exhausted;
        self.exhausted_until = next_reset;
        self.cooldown_until = None;
    }

    /// 尝试恢复（冷却或耗尽）
    pub fn recover_if_ready(&mut self) -> bool {
        let now = Utc::now();
        match self.status {
            AccountStatus::Cooldown if self.cooldown_until.map(|t| now >= t).unwrap_or(true) => {
                self.status = AccountStatus::Active;
                self.cooldown_until = None;
                true
            }
            AccountStatus::Exhausted if self.exhausted_until.map(|t| now >= t).unwrap_or(false) => {
                self.status = AccountStatus::Active;
                self.exhausted_until = None;
                true
            }
            _ => false,
        }
    }

    /// 启用账号
    pub fn enable(&mut self) {
        if self.status == AccountStatus::Disabled {
            self.status = AccountStatus::Active;
            self.cooldown_until = None;
            self.exhausted_until = None;
        }
    }

    /// 禁用账号
    pub fn disable(&mut self) {
        self.status = AccountStatus::Disabled;
        self.cooldown_until = None;
        self.exhausted_until = None;
    }
}
