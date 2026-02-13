//! 账号池管理器

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::http_client::ProxyConfig;
use crate::kiro::provider::KiroProvider;
use crate::kiro::token_manager::TokenManager;
use crate::model::config::Config;

use super::account::{Account, AccountStatus};
use super::strategy::SelectionStrategy;
use super::usage::{RequestLog, RequestLogger, RequestStats, UsageLimits};

/// 账号存储文件名
const ACCOUNTS_FILE: &str = "accounts.json";
/// 请求记录存储文件名
const LOGS_FILE: &str = "request_logs.json";
/// 配额缓存存储文件名
const USAGE_CACHE_FILE: &str = "usage_cache.json";

/// 账号池管理器
pub struct AccountPool {
    /// 账号列表
    accounts: RwLock<HashMap<String, Account>>,
    /// Token 管理器缓存
    token_managers: RwLock<HashMap<String, Arc<tokio::sync::Mutex<TokenManager>>>>,
    /// Provider 缓存（每账号一个，避免每请求创建 Client）
    providers: RwLock<HashMap<String, Arc<KiroProvider>>>,
    /// 选择策略
    strategy: RwLock<SelectionStrategy>,
    /// 轮询索引
    round_robin_index: RwLock<usize>,
    /// 顺序耗尽策略当前账号
    sequential_current_id: RwLock<Option<String>>,
    /// 全局配置
    config: Config,
    /// 代理配置
    proxy: Option<ProxyConfig>,
    /// 数据存储目录
    data_dir: Option<PathBuf>,
    /// 请求记录器
    request_logger: RwLock<RequestLogger>,
    /// 账号配额缓存
    usage_cache: RwLock<HashMap<String, UsageLimits>>,
}

/// 账号池选择结果
pub struct SelectedAccount {
    pub id: String,
    pub name: String,
    pub provider: Arc<KiroProvider>,
}

impl AccountPool {
    /// 创建新的账号池
    #[allow(dead_code)]
    pub fn new(config: Config, proxy: Option<ProxyConfig>) -> Self {
        Self {
            accounts: RwLock::new(HashMap::new()),
            token_managers: RwLock::new(HashMap::new()),
            providers: RwLock::new(HashMap::new()),
            strategy: RwLock::new(SelectionStrategy::default()),
            round_robin_index: RwLock::new(0),
            sequential_current_id: RwLock::new(None),
            config,
            proxy,
            data_dir: None,
            request_logger: RwLock::new(RequestLogger::default()),
            usage_cache: RwLock::new(HashMap::new()),
        }
    }

    /// 创建带持久化存储的账号池
    pub fn with_data_dir(config: Config, proxy: Option<ProxyConfig>, data_dir: PathBuf) -> Self {
        Self {
            accounts: RwLock::new(HashMap::new()),
            token_managers: RwLock::new(HashMap::new()),
            providers: RwLock::new(HashMap::new()),
            strategy: RwLock::new(SelectionStrategy::default()),
            round_robin_index: RwLock::new(0),
            sequential_current_id: RwLock::new(None),
            config,
            proxy,
            data_dir: Some(data_dir),
            request_logger: RwLock::new(RequestLogger::default()),
            usage_cache: RwLock::new(HashMap::new()),
        }
    }

    /// 从文件加载账号
    pub async fn load_from_file(&self) -> anyhow::Result<usize> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(0);
        };

        let file_path = data_dir.join(ACCOUNTS_FILE);
        if !file_path.exists() {
            return Ok(0);
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let stored: Vec<StoredAccount> = serde_json::from_str(&content)?;

        let mut count = 0;
        let mut migrated_invalid = 0;
        for stored_account in stored {
            if stored_account.status == AccountStatus::Invalid {
                migrated_invalid += 1;
            }
            let account = stored_account.into_account();
            if let Err(e) = self.add_account_internal(account).await {
                tracing::warn!("加载账号失败: {}", e);
            } else {
                count += 1;
            }
        }

        if migrated_invalid > 0 {
            tracing::warn!(
                "检测到 {} 个历史 invalid 账号，已自动迁移为 disabled",
                migrated_invalid
            );
            // 写回持久化，避免重启后重复迁移
            self.save_to_file().await?;
        }

        tracing::info!("从文件加载了 {} 个账号", count);
        Ok(count)
    }

    /// 保存账号到文件
    pub async fn save_to_file(&self) -> anyhow::Result<()> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(());
        };

        // 确保目录存在
        tokio::fs::create_dir_all(data_dir).await?;

        let accounts = self.accounts.read().await;
        let stored: Vec<StoredAccount> =
            accounts.values().map(StoredAccount::from_account).collect();

        let content = serde_json::to_string_pretty(&stored)?;
        let file_path = data_dir.join(ACCOUNTS_FILE);
        tokio::fs::write(&file_path, content).await?;

        tracing::debug!("已保存 {} 个账号到文件", stored.len());
        Ok(())
    }

    /// 内部添加账号（不保存文件）
    async fn add_account_internal(&self, account: Account) -> anyhow::Result<()> {
        let id = account.id.clone();
        let credentials = account.credentials.clone();

        // 创建 TokenManager
        let token_manager = TokenManager::new(self.config.clone(), credentials, self.proxy.clone());

        let tm = Arc::new(tokio::sync::Mutex::new(token_manager));
        let provider = Arc::new(KiroProvider::with_shared_token_manager(
            tm.clone(),
            self.proxy.clone(),
        ));

        let mut accounts = self.accounts.write().await;
        let mut managers = self.token_managers.write().await;
        let mut providers = self.providers.write().await;

        accounts.insert(id.clone(), account);
        managers.insert(id.clone(), tm);
        providers.insert(id, provider);

        Ok(())
    }

    /// 添加账号
    pub async fn add_account(&self, account: Account) -> anyhow::Result<()> {
        self.add_account_internal(account).await?;
        self.save_to_file().await?;
        Ok(())
    }

    /// 验证凭证是否有效（尝试刷新 token）
    ///
    /// 返回 Ok(()) 表示凭证有效，Err 表示凭证无效
    pub async fn validate_credentials(
        &self,
        credentials: &crate::kiro::model::credentials::KiroCredentials,
    ) -> anyhow::Result<()> {
        // 创建临时 TokenManager 进行验证
        let mut token_manager =
            TokenManager::new(self.config.clone(), credentials.clone(), self.proxy.clone());

        // 尝试获取有效 token（会触发刷新）
        token_manager.ensure_valid_token().await?;

        Ok(())
    }

    /// 添加账号（带验证）
    ///
    /// 先验证凭证是否有效，有效才添加
    pub async fn add_account_with_validation(&self, account: Account) -> anyhow::Result<()> {
        // 先验证凭证
        self.validate_credentials(&account.credentials).await?;

        // 验证通过，添加账号
        self.add_account_internal(account).await?;
        self.save_to_file().await?;
        Ok(())
    }

    /// 移除账号
    pub async fn remove_account(&self, id: &str) -> Option<Account> {
        let mut accounts = self.accounts.write().await;
        let mut managers = self.token_managers.write().await;
        let mut providers = self.providers.write().await;
        let mut sequential_current_id = self.sequential_current_id.write().await;
        let mut usage_cache = self.usage_cache.write().await;

        managers.remove(id);
        providers.remove(id);
        usage_cache.remove(id);
        let removed = accounts.remove(id);
        if sequential_current_id.as_deref() == Some(id) {
            *sequential_current_id = None;
        }

        // 保存到文件
        drop(accounts);
        drop(managers);
        drop(providers);
        drop(sequential_current_id);
        drop(usage_cache);
        if let Err(e) = self.save_to_file().await {
            tracing::warn!("保存账号文件失败: {}", e);
        }
        self.save_usage_cache().await;

        removed
    }

    /// 获取所有账号（不含凭证）
    pub async fn list_accounts(&self) -> Vec<Account> {
        let accounts = self.accounts.read().await;
        accounts.values().cloned().collect()
    }

    /// 设置选择策略
    pub async fn set_strategy(&self, strategy: SelectionStrategy) {
        *self.strategy.write().await = strategy;
        *self.sequential_current_id.write().await = None;
    }

    /// 获取当前策略
    pub async fn get_strategy(&self) -> SelectionStrategy {
        *self.strategy.read().await
    }

    /// 选择一个可用账号并获取其 TokenManager
    pub async fn select_account(&self) -> Option<SelectedAccount> {
        let strategy = *self.strategy.read().await;
        if strategy == SelectionStrategy::SequentialExhaust {
            return self.select_account_sequential_exhaust().await;
        }

        // 先用读锁快速收集可用账号（避免长时间持有写锁）
        let available: Vec<(String, u64)> = {
            let accounts = self.accounts.read().await;
            accounts
                .iter()
                .filter(|(_, a)| a.is_available())
                .map(|(id, a)| (id.clone(), a.request_count))
                .collect()
        };

        if available.is_empty() {
            return None;
        }

        // 根据策略选出候选 id（不持有 accounts 锁）
        let candidate_id = match strategy {
            SelectionStrategy::RoundRobin => {
                let mut index = self.round_robin_index.write().await;
                let id = available[*index % available.len()].0.clone();
                *index = (*index + 1) % available.len();
                id
            }
            SelectionStrategy::Random => {
                let idx = fastrand::usize(..available.len());
                available[idx].0.clone()
            }
            SelectionStrategy::LeastUsed => available
                .iter()
                .min_by_key(|(_, count)| *count)
                .map(|(id, _)| id.clone())
                .unwrap_or_else(|| available[0].0.clone()),
            SelectionStrategy::SequentialExhaust => unreachable!(),
        };

        // 用写锁记录使用，并最终确认选中的账号
        let (selected_id, selected_name) = {
            let mut accounts = self.accounts.write().await;

            if let Some(account) = accounts.get_mut(&candidate_id) {
                if account.is_available() {
                    account.record_use();
                    (candidate_id.clone(), account.name.clone())
                } else {
                    // 候选账号在并发下变为不可用，退化为找一个可用账号
                    let mut picked: Option<(String, String)> = None;
                    for (id, a) in accounts.iter_mut() {
                        if a.is_available() {
                            a.record_use();
                            picked = Some((id.clone(), a.name.clone()));
                            break;
                        }
                    }
                    picked?
                }
            } else {
                // 候选账号已被删除，退化为找一个可用账号
                let mut picked: Option<(String, String)> = None;
                for (id, a) in accounts.iter_mut() {
                    if a.is_available() {
                        a.record_use();
                        picked = Some((id.clone(), a.name.clone()));
                        break;
                    }
                }
                picked?
            }
        };

        let provider = {
            let providers = self.providers.read().await;
            providers.get(&selected_id).cloned()?
        };

        Some(SelectedAccount {
            id: selected_id,
            name: selected_name,
            provider,
        })
    }

    /// 顺序耗尽策略选账号：当前可用则持续使用，不可用才切下一个
    async fn select_account_sequential_exhaust(&self) -> Option<SelectedAccount> {
        let current_id = self.sequential_current_id.read().await.clone();

        // 快照：稳定顺序 + 是否可选（包含 cached quota 可用性）
        let (ordered_ids, selectable_map, cached_exhausted_ids) = {
            let accounts = self.accounts.read().await;
            let usage_cache = self.usage_cache.read().await;

            let mut ordered_accounts: Vec<&Account> = accounts.values().collect();
            ordered_accounts.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            });

            let cached_exhausted_ids: HashSet<String> = usage_cache
                .iter()
                .filter(|(_, usage)| usage.available <= 0.0)
                .map(|(id, _)| id.clone())
                .collect();

            let ordered_ids: Vec<String> = ordered_accounts.iter().map(|a| a.id.clone()).collect();
            let selectable_map: HashMap<String, bool> = ordered_accounts
                .iter()
                .map(|a| {
                    (
                        a.id.clone(),
                        a.is_available() && !cached_exhausted_ids.contains(&a.id),
                    )
                })
                .collect();

            (ordered_ids, selectable_map, cached_exhausted_ids)
        };

        if ordered_ids.is_empty() {
            return None;
        }

        // 构建搜索顺序：当前可用就只尝试当前；否则从下一个开始循环
        let search_order: Vec<String> = if let Some(curr) = &current_id {
            if selectable_map.get(curr).copied().unwrap_or(false) {
                vec![curr.clone()]
            } else if let Some(pos) = ordered_ids.iter().position(|id| id == curr) {
                (0..ordered_ids.len())
                    .map(|i| ordered_ids[(pos + 1 + i) % ordered_ids.len()].clone())
                    .collect()
            } else {
                ordered_ids.clone()
            }
        } else {
            ordered_ids.clone()
        };

        let selected = {
            let mut accounts = self.accounts.write().await;
            let mut picked: Option<(String, String)> = None;

            for id in search_order {
                if cached_exhausted_ids.contains(&id) {
                    continue;
                }
                if let Some(account) = accounts.get_mut(&id) {
                    if account.is_available() {
                        account.record_use();
                        picked = Some((id, account.name.clone()));
                        break;
                    }
                }
            }

            picked
        };

        let Some((selected_id, selected_name)) = selected else {
            *self.sequential_current_id.write().await = None;
            return None;
        };

        *self.sequential_current_id.write().await = Some(selected_id.clone());

        let provider = {
            let providers = self.providers.read().await;
            providers.get(&selected_id).cloned()?
        };

        Some(SelectedAccount {
            id: selected_id,
            name: selected_name,
            provider,
        })
    }

    /// 启用账号
    pub async fn enable_account(&self, id: &str) -> bool {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.enable();
            drop(accounts);
            let _ = self.save_to_file().await;
            true
        } else {
            false
        }
    }

    /// 禁用账号
    pub async fn disable_account(&self, id: &str) -> bool {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.disable();
            drop(accounts);
            let _ = self.save_to_file().await;
            true
        } else {
            false
        }
    }

    /// 记录账号错误
    pub async fn record_error(&self, id: &str, is_rate_limit: bool) {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.record_error(is_rate_limit);
            tracing::info!(
                "账号 {} 记录错误，限流: {}，当前错误数: {}，状态: {:?}",
                id,
                is_rate_limit,
                account.error_count,
                account.status
            );
            drop(accounts);
            let _ = self.save_to_file().await;
        }
    }

    /// 标记账号为失效（自动禁用）
    pub async fn mark_invalid(&self, id: &str) {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.mark_invalid();
            tracing::warn!("账号 {} 已检测为失效，已自动禁用", id);
            drop(accounts);
            let _ = self.save_to_file().await;
        }
    }

    /// 标记账号配额耗尽
    pub async fn mark_exhausted(
        &self,
        id: &str,
        next_reset: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(id) {
            account.mark_exhausted(next_reset);
            tracing::warn!("账号 {} 已标记为配额耗尽", id);
            drop(accounts);
            let _ = self.save_to_file().await;
        }
    }

    /// 扫描并恢复到期冷却账号（15分钟任务）
    pub async fn recover_cooldown_accounts(&self) -> usize {
        let mut accounts = self.accounts.write().await;
        let mut recovered = 0usize;

        for account in accounts.values_mut() {
            if account.status == AccountStatus::Cooldown && account.recover_if_ready() {
                recovered += 1;
            }
        }

        drop(accounts);
        if recovered > 0 {
            let _ = self.save_to_file().await;
        }
        recovered
    }

    /// 扫描配额耗尽账号并尝试恢复（1小时任务）
    ///
    /// 返回：(成功恢复数, 检查总数)
    pub async fn refresh_exhausted_accounts(&self) -> (usize, usize) {
        let exhausted_ids: Vec<String> = {
            let accounts = self.accounts.read().await;
            accounts
                .values()
                .filter(|a| a.status == AccountStatus::Exhausted)
                .map(|a| a.id.clone())
                .collect()
        };

        let mut recovered = 0usize;
        for id in &exhausted_ids {
            match self.refresh_account_usage(id).await {
                Ok(usage) => {
                    if usage.available > 0.0 {
                        let mut accounts = self.accounts.write().await;
                        if let Some(account) = accounts.get_mut(id) {
                            account.status = AccountStatus::Active;
                            account.exhausted_until = None;
                            recovered += 1;
                        }
                        drop(accounts);
                        let _ = self.save_to_file().await;
                    } else {
                        self.mark_exhausted(id, usage.next_reset).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("扫描配额耗尽账号 {} 失败: {}", id, e);
                }
            }
        }

        (recovered, exhausted_ids.len())
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> PoolStats {
        let accounts = self.accounts.read().await;

        let total = accounts.len();
        let active = accounts
            .values()
            .filter(|a| a.status == AccountStatus::Active)
            .count();
        let cooldown = accounts
            .values()
            .filter(|a| a.status == AccountStatus::Cooldown)
            .count();
        let exhausted = accounts
            .values()
            .filter(|a| a.status == AccountStatus::Exhausted)
            .count();
        let invalid = accounts
            .values()
            .filter(|a| a.status == AccountStatus::Invalid)
            .count();
        let disabled = accounts
            .values()
            .filter(|a| a.status == AccountStatus::Disabled)
            .count();
        let total_requests: u64 = accounts.values().map(|a| a.request_count).sum();
        let total_errors: u64 = accounts.values().map(|a| a.error_count).sum();

        PoolStats {
            total,
            active,
            cooldown,
            exhausted,
            invalid,
            disabled,
            total_requests,
            total_errors,
        }
    }

    /// 添加请求记录
    pub async fn add_request_log(&self, log: RequestLog) {
        let mut logger = self.request_logger.write().await;
        logger.add(log);

        // 异步保存到文件（不阻塞）
        if let Some(data_dir) = &self.data_dir {
            let logs = logger.get_all();
            let file_path = data_dir.join(LOGS_FILE);
            tokio::spawn(async move {
                if let Ok(content) = serde_json::to_string(&logs) {
                    let _ = tokio::fs::write(&file_path, content).await;
                }
            });
        }
    }

    /// 获取最近的请求记录
    pub async fn get_recent_logs(&self, n: usize) -> Vec<RequestLog> {
        let logger = self.request_logger.read().await;
        logger.get_recent(n)
    }

    /// 获取请求统计
    pub async fn get_request_stats(&self) -> RequestStats {
        let logger = self.request_logger.read().await;
        logger.get_stats()
    }

    /// 从文件加载请求记录
    pub async fn load_logs_from_file(&self) -> anyhow::Result<usize> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(0);
        };

        let file_path = data_dir.join(LOGS_FILE);
        if !file_path.exists() {
            return Ok(0);
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let mut logs: Vec<RequestLog> = serde_json::from_str(&content)?;

        // 只保留最新的 1000 条（如果超过的话）
        if logs.len() > 1000 {
            logs = logs.split_off(logs.len() - 1000);
        }

        let count = logs.len();
        let mut logger = self.request_logger.write().await;
        for log in logs {
            logger.add(log);
        }

        tracing::info!("从文件加载了 {} 条请求记录", count);
        Ok(count)
    }

    /// 获取账号配额（带缓存）
    pub async fn get_account_usage(&self, id: &str) -> Option<UsageLimits> {
        let cache = self.usage_cache.read().await;
        cache.get(id).cloned()
    }

    /// 刷新账号配额
    pub async fn refresh_account_usage(&self, id: &str) -> anyhow::Result<UsageLimits> {
        // 获取 TokenManager
        let managers = self.token_managers.read().await;
        let tm = managers
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("账号不存在"))?;

        // 获取 access_token
        let mut tm_guard = tm.lock().await;
        let token = match tm_guard.ensure_valid_token().await {
            Ok(t) => t,
            Err(e) => {
                let error_msg = e.to_string();
                // 检测 403/suspended 错误，自动禁用账号
                if error_msg.contains("403")
                    || error_msg.contains("suspended")
                    || error_msg.contains("SUSPENDED")
                {
                    drop(tm_guard);
                    drop(managers);
                    self.mark_invalid(id).await;
                    tracing::warn!("账号 {} 获取 token 失败，已自动禁用: {}", id, error_msg);
                }
                return Err(e);
            }
        };
        drop(tm_guard);
        drop(managers);

        // 调用 API 获取配额
        let usage = match super::usage::check_usage_limits(&token).await {
            Ok(u) => u,
            Err(e) => {
                let error_msg = e.to_string();
                let is_suspended = error_msg.contains("403")
                    || error_msg.contains("suspended")
                    || error_msg.contains("SUSPENDED");
                let is_quota_exceeded = error_msg.contains("402")
                    || error_msg.contains("Payment Required")
                    || error_msg.contains("MONTHLY_REQUEST_COUNT")
                    || error_msg.contains("reached the limit");

                if is_suspended {
                    self.mark_invalid(id).await;
                    tracing::warn!("账号 {} 获取配额失败，已自动禁用: {}", id, error_msg);
                } else if is_quota_exceeded {
                    self.mark_exhausted(id, None).await;
                    tracing::warn!("账号 {} 获取配额失败，已标记为配额耗尽: {}", id, error_msg);
                }
                return Err(e);
            }
        };

        // 更新缓存
        let mut cache = self.usage_cache.write().await;
        cache.insert(id.to_string(), usage.clone());
        drop(cache);

        // 同步账号状态：有额度则恢复，额度耗尽则标记为 Exhausted
        if usage.available > 0.0 {
            let mut accounts = self.accounts.write().await;
            if let Some(account) = accounts.get_mut(id) {
                if account.status == AccountStatus::Exhausted {
                    account.status = AccountStatus::Active;
                    account.exhausted_until = None;
                }
            }
            drop(accounts);
            let _ = self.save_to_file().await;
        } else {
            self.mark_exhausted(id, usage.next_reset).await;
        }

        // 保存到文件
        self.save_usage_cache().await;

        Ok(usage)
    }

    /// 保存配额缓存到文件
    async fn save_usage_cache(&self) {
        if let Some(data_dir) = &self.data_dir {
            let cache = self.usage_cache.read().await;
            let file_path = data_dir.join(USAGE_CACHE_FILE);
            if let Ok(content) = serde_json::to_string(&*cache) {
                let _ = tokio::fs::write(&file_path, content).await;
            }
        }
    }

    /// 从文件加载配额缓存
    pub async fn load_usage_cache(&self) -> anyhow::Result<usize> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(0);
        };

        let file_path = data_dir.join(USAGE_CACHE_FILE);
        if !file_path.exists() {
            return Ok(0);
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let loaded: HashMap<String, UsageLimits> = serde_json::from_str(&content)?;

        let count = loaded.len();
        let mut cache = self.usage_cache.write().await;
        *cache = loaded;

        tracing::info!("从文件加载了 {} 个配额缓存", count);
        Ok(count)
    }

    /// 刷新所有账号配额
    pub async fn refresh_all_usage(&self) -> Vec<(String, Result<UsageLimits, String>)> {
        let accounts = self.accounts.read().await;
        let ids: Vec<String> = accounts.keys().cloned().collect();
        drop(accounts);

        let mut results = Vec::new();
        for id in ids {
            let result = match self.refresh_account_usage(&id).await {
                Ok(usage) => Ok(usage),
                Err(e) => Err(e.to_string()),
            };
            results.push((id, result));
        }
        results
    }

    /// 获取所有账号配额缓存
    pub async fn get_all_usage(&self) -> HashMap<String, UsageLimits> {
        let cache = self.usage_cache.read().await;
        cache.clone()
    }
}

/// 账号池统计
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolStats {
    pub total: usize,
    pub active: usize,
    pub cooldown: usize,
    pub exhausted: usize,
    pub invalid: usize,
    pub disabled: usize,
    pub total_requests: u64,
    pub total_errors: u64,
}

/// 用于持久化存储的账号结构
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StoredAccount {
    id: String,
    name: String,
    status: super::account::AccountStatus,
    request_count: u64,
    error_count: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    exhausted_until: Option<chrono::DateTime<chrono::Utc>>,
    // 凭证信息
    refresh_token: Option<String>,
    auth_method: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    profile_arn: Option<String>,
}

impl StoredAccount {
    fn from_account(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            name: account.name.clone(),
            status: account.status,
            request_count: account.request_count,
            error_count: account.error_count,
            created_at: account.created_at,
            exhausted_until: account.exhausted_until,
            refresh_token: account.credentials.refresh_token.clone(),
            auth_method: account.credentials.auth_method.clone(),
            client_id: account.credentials.client_id.clone(),
            client_secret: account.credentials.client_secret.clone(),
            profile_arn: account.credentials.profile_arn.clone(),
        }
    }

    fn into_account(self) -> Account {
        use crate::kiro::model::credentials::KiroCredentials;

        let credentials = KiroCredentials {
            access_token: None,
            refresh_token: self.refresh_token,
            profile_arn: self.profile_arn,
            expires_at: Some("2000-01-01T00:00:00Z".to_string()),
            auth_method: self.auth_method,
            client_id: self.client_id,
            client_secret: self.client_secret,
        };

        let status = if self.status == AccountStatus::Invalid {
            AccountStatus::Disabled
        } else {
            self.status
        };

        Account {
            id: self.id,
            name: self.name,
            credentials,
            status,
            request_count: self.request_count,
            error_count: self.error_count,
            last_used_at: None,
            cooldown_until: None,
            exhausted_until: self.exhausted_until,
            created_at: self.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::model::credentials::KiroCredentials;
    use chrono::{Duration, Utc};

    fn test_usage(available: f64) -> UsageLimits {
        UsageLimits {
            resource_type: "CREDIT".to_string(),
            usage_limit: 100.0,
            current_usage: 100.0 - available,
            available,
            next_reset: None,
            free_trial: None,
            user_email: None,
            subscription_type: None,
        }
    }

    async fn build_two_account_pool() -> AccountPool {
        let pool = AccountPool::new(Config::default(), None);

        let mut acc1 = Account::new("a", "A", KiroCredentials::default());
        acc1.created_at = Utc::now() - Duration::minutes(2);

        let mut acc2 = Account::new("b", "B", KiroCredentials::default());
        acc2.created_at = Utc::now() - Duration::minutes(1);

        pool.add_account(acc1).await.unwrap();
        pool.add_account(acc2).await.unwrap();
        pool.set_strategy(SelectionStrategy::SequentialExhaust)
            .await;
        pool
    }

    #[tokio::test]
    async fn test_sequential_exhaust_sticky_then_switch() {
        let pool = build_two_account_pool().await;

        let first = pool.select_account().await.unwrap();
        assert_eq!(first.id, "a");

        let second = pool.select_account().await.unwrap();
        assert_eq!(second.id, "a");

        assert!(pool.disable_account("a").await);
        let third = pool.select_account().await.unwrap();
        assert_eq!(third.id, "b");
    }

    #[tokio::test]
    async fn test_sequential_exhaust_no_preempt_after_recovery() {
        let pool = build_two_account_pool().await;

        let first = pool.select_account().await.unwrap();
        assert_eq!(first.id, "a");

        pool.mark_exhausted("a", Some(Utc::now() + Duration::hours(1)))
            .await;
        let second = pool.select_account().await.unwrap();
        assert_eq!(second.id, "b");

        {
            let mut accounts = pool.accounts.write().await;
            let acc = accounts.get_mut("a").unwrap();
            acc.status = AccountStatus::Active;
            acc.exhausted_until = None;
        }

        let third = pool.select_account().await.unwrap();
        assert_eq!(third.id, "b");

        assert!(pool.disable_account("b").await);
        let fourth = pool.select_account().await.unwrap();
        assert_eq!(fourth.id, "a");
    }

    #[tokio::test]
    async fn test_sequential_exhaust_skips_cached_zero_quota() {
        let pool = build_two_account_pool().await;
        {
            let mut cache = pool.usage_cache.write().await;
            cache.insert("a".to_string(), test_usage(0.0));
        }

        let selected = pool.select_account().await.unwrap();
        assert_eq!(selected.id, "b");
    }

    #[test]
    fn test_stored_account_invalid_migrates_to_disabled() {
        let stored = StoredAccount {
            id: "x".to_string(),
            name: "legacy".to_string(),
            status: AccountStatus::Invalid,
            request_count: 0,
            error_count: 0,
            created_at: Utc::now(),
            exhausted_until: None,
            refresh_token: Some("r".to_string()),
            auth_method: Some("social".to_string()),
            client_id: None,
            client_secret: None,
            profile_arn: None,
        };

        let account = stored.into_account();
        assert_eq!(account.status, AccountStatus::Disabled);
    }
}
