// transport-core/src/failover.rs

//! Multi-endpoint failover для gateway-подключения.
//!
//! Принципы (AMO_SPEC + reviewer):
//! - Deterministic priority list (не dynamic discovery)
//! - Failover только по timeout/connection-refused/DNS-fail/TLS-handshake-fail
//!   (НЕ по protocol error mid-handshake — иначе DPI fingerprint). Этот контракт
//!   обеспечивается вызывающей стороной (proxy.rs, A3) — здесь только механика.
//! - Cooldown ПЕР-ENDPOINT (не глобальный), минимум 60s
//! - Единый мьютекс на всё состояние (idx + states + last_switch) — устраняет
//!   TOCTOU-гонку между чтением индекса и переключением (reviewer A2 critical).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::endpoint::EndpointConfig;

/// Min interval между повторными попытками ОДНОГО endpoint (per-endpoint cooldown).
pub const FAILOVER_MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Сколько подряд неудач на текущем endpoint триггерит переключение.
pub const FAILURE_THRESHOLD: u32 = 3;

/// Абстракция времени — позволяет юнит-тестам продвигать "часы" без реального
/// sleep(60s). В проде используется `SystemClock`.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Управляемые "часы" для тестов. Instant нельзя сконструировать произвольно
/// в stable Rust, поэтому FakeClock хранит базовый Instant::now() и сдвигает
/// его на явно заданную Duration через advance().
pub struct FakeClock {
    current: Mutex<Instant>,
}

impl FakeClock {
    pub fn new() -> Self {
        Self { current: Mutex::new(Instant::now()) }
    }

    pub fn advance(&self, d: Duration) {
        let mut c = self.current.lock().unwrap();
        *c += d;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        *self.current.lock().unwrap()
    }
}

struct EndpointState {
    consecutive_failures: u32,
    last_failure: Option<Instant>,
}

impl EndpointState {
    fn fresh() -> Self {
        Self { consecutive_failures: 0, last_failure: None }
    }

    fn in_cooldown(&self, now: Instant) -> bool {
        match self.last_failure {
            Some(t) => now.duration_since(t) < FAILOVER_MIN_INTERVAL,
            None => false,
        }
    }
}

/// Всё изменяемое состояние под ОДНИМ мьютексом — устраняет TOCTOU между
/// чтением текущего индекса и попыткой переключения (reviewer A2 critical #2/#3).
struct Inner {
    current_idx: usize,
    states: Vec<EndpointState>,
    last_switch_system_time: Option<SystemTime>,
}

/// Failover manager: deterministic priority list + per-endpoint cooldown.
pub struct FailoverManager {
    endpoints: Vec<EndpointConfig>,
    inner: Mutex<Inner>,
    clock: Arc<dyn Clock>,
}

impl FailoverManager {
    /// Создаёт менеджер с системными часами (прод-путь).
    /// Валидация списка (пусто/>16) уже сделана в endpoint::parse_endpoints (A1),
    /// здесь не дублируется — принимает любой непустой Vec как есть.
    pub fn new(endpoints: Vec<EndpointConfig>) -> Self {
        Self::with_clock(endpoints, Arc::new(SystemClock))
    }

    /// Создаёт менеджер с инжектируемыми часами — используется в тестах для
    /// проверки истечения 60s cooldown без реального ожидания.
    pub fn with_clock(endpoints: Vec<EndpointConfig>, clock: Arc<dyn Clock>) -> Self {
        let states = endpoints.iter().map(|_| EndpointState::fresh()).collect();
        Self {
            endpoints,
            inner: Mutex::new(Inner { current_idx: 0, states, last_switch_system_time: None }),
            clock,
        }
    }

    /// Возвращает текущий активный endpoint (клон — дешёво, структура маленькая).
    pub fn current_endpoint(&self) -> EndpointConfig {
        let inner = self.inner.lock().unwrap();
        self.endpoints[inner.current_idx].clone()
    }

    pub fn current_index(&self) -> usize {
        self.inner.lock().unwrap().current_idx
    }

    pub fn len(&self) -> usize {
        self.endpoints.len()
    }

    /// Регистрирует неудачу на ТЕКУЩЕМ endpoint (timeout/refused/DNS-fail/TLS-fail).
    /// НЕ вызывается на protocol error mid-handshake — это ответственность proxy.rs (A3).
    /// После FAILURE_THRESHOLD подряд неудач автоматически переключается.
    /// Вся операция — под ОДНИМ захватом мьютекса (без повторного лока), что
    /// устраняет TOCTOU-гонку из ревизии.
    pub fn report_failure(&self) {
        let now = self.clock.now();
        let mut inner = self.inner.lock().unwrap();

        let idx = inner.current_idx;
        inner.states[idx].consecutive_failures += 1;
        inner.states[idx].last_failure = Some(now);

        if inner.states[idx].consecutive_failures >= FAILURE_THRESHOLD {
            self.try_switch_locked(&mut inner, now);
        }
    }

    /// Сбрасывает счётчик неудач текущего endpoint после успешного подключения.
    pub fn report_success(&self) {
        let mut inner = self.inner.lock().unwrap();
        let idx = inner.current_idx;
        inner.states[idx].consecutive_failures = 0;
        inner.states[idx].last_failure = None;
    }

    /// Публичная версия try_switch — берёт лок сама (для вызова из тестов/proxy.rs
    /// напрямую, минуя report_failure).
    pub fn try_switch(&self) -> bool {
        let now = self.clock.now();
        let mut inner = self.inner.lock().unwrap();
        self.try_switch_locked(&mut inner, now)
    }

    /// Внутренняя реализация переключения, работающая на уже захваченном локе.
    /// Обходит список по кругу от текущего+1 (детерминированный приоритет из AMO_SPEC).
    /// При успешном переключении сбрасывает consecutive_failures ПОКИДАЕМОГО
    /// endpoint (reviewer A2 critical #1) — иначе после cooldown первая же
    /// неудача мгновенно вышвырнет его снова, не дав нормального шанса.
    /// last_failure покидаемого endpoint СОХРАНЯЕТСЯ — иначе cooldown-таймер
    /// потеряется и endpoint станет доступен немедленно.
    fn try_switch_locked(&self, inner: &mut Inner, now: Instant) -> bool {
        let len = self.endpoints.len();
        let old_idx = inner.current_idx;

        for step in 1..=len {
            let candidate = (old_idx + step) % len;
            if !inner.states[candidate].in_cooldown(now) {
                inner.states[old_idx].consecutive_failures = 0;
                inner.current_idx = candidate;
                inner.last_switch_system_time = Some(SystemTime::now());
                return true;
            }
        }
        // Все endpoints в cooldown — остаёмся на месте (fail-closed на уровне A3/A4).
        false
    }

    /// True если endpoint с данным host:port сейчас в cooldown.
    pub fn is_in_cooldown(&self, host: &str, port: u16) -> bool {
        let now = self.clock.now();
        let inner = self.inner.lock().unwrap();
        self.endpoints
            .iter()
            .position(|e| e.host == host && e.port == port)
            .map(|i| inner.states[i].in_cooldown(now))
            .unwrap_or(false)
    }

    /// Текущее число подряд неудач на активном endpoint (для transport_get_status).
    pub fn failure_count(&self) -> u32 {
        let inner = self.inner.lock().unwrap();
        inner.states[inner.current_idx].consecutive_failures
    }

    /// Unix ms последнего переключения (0 если переключений не было).
    /// Берётся из SystemTime, зафиксированного в момент switch — точное
    /// значение, а не аппроксимация "сейчас" (исправление из ревизии A2).
    pub fn last_switch_unix_ms(&self) -> u64 {
        let inner = self.inner.lock().unwrap();
        match inner.last_switch_system_time {
            None => 0,
            Some(t) => t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn ep(n: u8) -> EndpointConfig {
        EndpointConfig {
            host: format!("gw{n}.example.com"),
            port: 443,
            sni: "cloudflare.com".to_string(),
            server_pub: "aa".repeat(32),
            psk: "bb".repeat(32),
        }
    }

    fn three_endpoints() -> Vec<EndpointConfig> {
        vec![ep(1), ep(2), ep(3)]
    }

    #[test]
    fn current_endpoint_initial() {
        let mgr = FailoverManager::new(three_endpoints());
        assert_eq!(mgr.current_endpoint().host, "gw1.example.com");
        assert_eq!(mgr.current_index(), 0);
    }

    #[test]
    fn no_switch_below_threshold() {
        let mgr = FailoverManager::new(three_endpoints());
        for _ in 0..FAILURE_THRESHOLD - 1 {
            mgr.report_failure();
            assert_eq!(mgr.current_index(), 0, "не должно переключаться до threshold");
        }
    }

    #[test]
    fn switch_at_threshold() {
        let mgr = FailoverManager::new(three_endpoints());
        for _ in 0..FAILURE_THRESHOLD {
            mgr.report_failure();
        }
        assert_eq!(mgr.current_index(), 1);
    }

    #[test]
    fn per_endpoint_cooldown_not_global() {
        let mgr = FailoverManager::new(three_endpoints());
        for _ in 0..FAILURE_THRESHOLD {
            mgr.report_failure();
        }
        assert_eq!(mgr.current_index(), 1);
        assert!(mgr.is_in_cooldown("gw1.example.com", 443), "gw1 должен быть в cooldown");
        assert!(!mgr.is_in_cooldown("gw3.example.com", 443), "gw3 (нетронутый) НЕ в cooldown");
    }

    #[test]
    fn report_success_resets_failure_count() {
        let mgr = FailoverManager::new(three_endpoints());
        mgr.report_failure();
        mgr.report_failure();
        assert_eq!(mgr.failure_count(), 2);
        mgr.report_success();
        assert_eq!(mgr.failure_count(), 0);
    }

    #[test]
    fn all_in_cooldown_try_switch_returns_false_stays_put() {
        let mgr = FailoverManager::new(vec![ep(1), ep(2)]);
        for _ in 0..FAILURE_THRESHOLD {
            mgr.report_failure(); // -> idx 1
        }
        assert_eq!(mgr.current_index(), 1);
        for _ in 0..FAILURE_THRESHOLD {
            mgr.report_failure(); // idx1 fails, idx0 в cooldown -> нет здоровых кандидатов
        }
        assert_eq!(mgr.current_index(), 1, "остаёмся на месте, если все остальные в cooldown");
        assert!(!mgr.try_switch());
    }

    #[test]
    fn len_reports_correctly() {
        let mgr = FailoverManager::new(three_endpoints());
        assert_eq!(mgr.len(), 3);
    }

    // ── Новые тесты по правкам ревизора ──

    #[test]
    fn counter_resets_on_switch_so_endpoint_gets_fresh_chance() {
        // Reviewer A2 critical #1: после switch у покинутого endpoint счётчик
        // должен сброситься, иначе первая же неудача после возврата снова
        // мгновенно вышвырнет его без полного порога попыток.
        let mgr = FailoverManager::new(three_endpoints());
        for _ in 0..FAILURE_THRESHOLD {
            mgr.report_failure(); // endpoint 0 -> switch на 1
        }
        assert_eq!(mgr.current_index(), 1);

        // Внутреннее состояние endpoint 0 доступно только через try_switch/failure_count
        // текущего индекса, поэтому проверяем поведение: сломаем 1 и 2, вернувшись к 0
        // после того как cooldown у 0 истечёт (через FakeClock — см. следующий тест),
        // счётчик 0 должен начинаться с нуля, а не с 3.
        // Здесь фиксируем контракт напрямую через повторный сценарий с FakeClock.
        let clock = Arc::new(FakeClock::new());
        let mgr2 = FailoverManager::with_clock(three_endpoints(), clock.clone());
        for _ in 0..FAILURE_THRESHOLD {
            mgr2.report_failure(); // 0 -> 1, счётчик 0 сброшен
        }
        assert_eq!(mgr2.current_index(), 1);

        clock.advance(FAILOVER_MIN_INTERVAL + Duration::from_secs(1));
        // Ломаем 1 и 2, чтобы обойти круг и вернуться к 0 (у которого cooldown истёк).
        for _ in 0..FAILURE_THRESHOLD {
            mgr2.report_failure(); // 1 -> 2
        }
        assert_eq!(mgr2.current_index(), 2);
        for _ in 0..FAILURE_THRESHOLD {
            mgr2.report_failure(); // 2 -> 0 (cooldown у 0 истёк благодаря advance)
        }
        assert_eq!(mgr2.current_index(), 0);
        // Один-единственный failure на "свежем" endpoint 0 не должен сразу вышвырнуть его.
        mgr2.report_failure();
        assert_eq!(mgr2.current_index(), 0, "счётчик 0 был сброшен при первом switch — один failure не триггерит новый switch");
    }

    #[test]
    fn cooldown_expires_after_fake_clock_advance() {
        // Reviewer A2: без инъекции часов cooldown непроверяем в юнит-тестах.
        let clock = Arc::new(FakeClock::new());
        let mgr = FailoverManager::with_clock(vec![ep(1), ep(2)], clock.clone());

        for _ in 0..FAILURE_THRESHOLD {
            mgr.report_failure();
        }
        assert_eq!(mgr.current_index(), 1);
        assert!(mgr.is_in_cooldown("gw1.example.com", 443));

        clock.advance(FAILOVER_MIN_INTERVAL - Duration::from_secs(1));
        assert!(mgr.is_in_cooldown("gw1.example.com", 443), "cooldown ещё не истёк (59s < 60s)");

        clock.advance(Duration::from_secs(2));
        assert!(!mgr.is_in_cooldown("gw1.example.com", 443), "cooldown истёк (61s >= 60s)");
    }

    #[test]
    fn report_success_clears_cooldown() {
        let mgr = FailoverManager::new(three_endpoints());
        mgr.report_failure();
        mgr.report_failure();
        assert!(mgr.is_in_cooldown("gw1.example.com", 443), "после неудач должен быть cooldown");
        mgr.report_success();
        assert!(!mgr.is_in_cooldown("gw1.example.com", 443), "report_success должен снимать cooldown");
    }

    #[test]
    fn switch_order_is_deterministic_round_robin_from_current_plus_one() {
        let mgr = FailoverManager::new(three_endpoints());
        assert!(mgr.try_switch());
        assert_eq!(mgr.current_index(), 1, "первый switch без истории идёт на current+1");
        assert!(mgr.try_switch());
        assert_eq!(mgr.current_index(), 2);
        assert!(mgr.try_switch());
        assert_eq!(mgr.current_index(), 0, "обход по кругу");
    }

    #[test]
    fn concurrent_report_failure_and_success_no_panic_consistent_index() {
        // Стресс-тест на TOCTOU: N потоков зовут report_failure/report_success
        // параллельно — не должно быть паник, индекс всегда в допустимом диапазоне.
        let mgr = Arc::new(FailoverManager::new(three_endpoints()));
        let mut handles = Vec::new();

        for i in 0..8 {
            let mgr = mgr.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..200 {
                    if i % 2 == 0 {
                        mgr.report_failure();
                    } else {
                        mgr.report_success();
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let idx = mgr.current_index();
        assert!(idx < 3, "индекс должен оставаться в допустимом диапазоне после конкурентной нагрузки");
    }

    #[test]
    fn last_switch_unix_ms_zero_before_any_switch() {
        let mgr = FailoverManager::new(three_endpoints());
        assert_eq!(mgr.last_switch_unix_ms(), 0);
    }

    #[test]
    fn last_switch_unix_ms_nonzero_after_switch() {
        let mgr = FailoverManager::new(three_endpoints());
        for _ in 0..FAILURE_THRESHOLD {
            mgr.report_failure();
        }
        assert!(mgr.last_switch_unix_ms() > 0);
    }

    #[test]
    fn failure_count_tracks_current_endpoint_only() {
        let mgr = FailoverManager::new(three_endpoints());
        mgr.report_failure();
        assert_eq!(mgr.failure_count(), 1);
        for _ in 0..FAILURE_THRESHOLD {
            mgr.report_failure(); // в какой-то момент это переключит на endpoint 1
        }
        // failure_count теперь отражает НОВЫЙ текущий endpoint, счётчик которого = 0
        // (свежий, только что переключились).
        assert_eq!(mgr.failure_count(), 0);
    }
}