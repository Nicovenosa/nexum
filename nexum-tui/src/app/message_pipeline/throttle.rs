//! 自适应节流策略：控制 LLM 流式输出的渲染帧率。

use std::time::{Duration, Instant};

use super::PipelineAction;

/// 排空计划：控制每次 check_throttle 的消费量
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainPlan {
    /// 正常模式：提交一行（单次 RebuildAll）
    Single,
    /// 积压模式：一次性排空所有积压行（单次 RebuildAll 含全部内容）
    Batch,
}

/// 分块模式（内部状态）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkingMode {
    /// 平滑模式：逐行提交
    Smooth,
    /// 追赶模式：批量排空
    CatchUp,
}

/// 自适应分块策略：根据队列压力在 Smooth/CatchUp 模式间动态切换。
///
/// Smooth 模式（默认）：每次 tick 提交一行，保证流畅感。
/// CatchUp 模式：队列积压时一次性排空，快速收敛显示。
///
/// 进入 CatchUp 条件（满足任一）：
/// - 队列深度 >= `queue_depth_threshold`（默认 8 行）
/// - 最老行年龄 >= `oldest_age_threshold`（默认 120ms）
///
/// 退出 CatchUp 条件（同时满足）：
/// - 队列深度 <= `exit_depth`（默认 2 行）
/// - 最老行年龄 <= `exit_age`（默认 40ms）
pub(crate) struct AdaptiveChunkingPolicy {
    /// 当前是否处于 CatchUp 模式
    pub(crate) mode: ChunkingMode,
    /// 累积的未消费行数（按换行符计）
    pub(crate) pending_lines: usize,
    /// 首个未消费 chunk 的到达时间（用于计算最老行年龄）
    pub(crate) oldest_chunk_at: Option<Instant>,
    /// 进入 CatchUp 的队列深度阈值
    queue_depth_threshold: usize,
    /// 进入 CatchUp 的最老行年龄阈值
    oldest_age_threshold: Duration,
    /// 退出 CatchUp 的队列深度阈值
    exit_depth: usize,
    /// 退出 CatchUp 的最老行年龄阈值
    exit_age: Duration,
}

impl AdaptiveChunkingPolicy {
    /// 使用默认参数创建策略
    pub(crate) fn new() -> Self {
        Self {
            mode: ChunkingMode::Smooth,
            pending_lines: 0,
            oldest_chunk_at: None,
            queue_depth_threshold: 8,
            oldest_age_threshold: Duration::from_millis(120),
            exit_depth: 2,
            exit_age: Duration::from_millis(40),
        }
    }

    /// 通知策略有新的 chunk 到达。
    /// 按换行符统计行数，并记录首个 chunk 的时间戳。
    pub(crate) fn on_chunk(&mut self, chunk: &str) {
        let new_lines = chunk.lines().count().max(1);
        self.pending_lines += new_lines;
        if self.oldest_chunk_at.is_none() {
            self.oldest_chunk_at = Some(Instant::now());
        }
    }

    /// 通知策略有新的推理 chunk 到达（同样累积压力）
    pub(crate) fn on_reasoning_chunk(&mut self) {
        self.pending_lines += 1;
        if self.oldest_chunk_at.is_none() {
            self.oldest_chunk_at = Some(Instant::now());
        }
    }

    /// 检查当前是否应该触发重绘，若触发则返回 DrainPlan。
    ///
    /// 策略逻辑：
    /// - Smooth 模式：检查基础节流间隔（最小 16ms，约 60fps），满足则返回 Single
    /// - CatchUp 模式：立即返回 Batch，无节流间隔限制
    /// - 每次调用检查是否需要模式切换
    pub(crate) fn check(&mut self) -> Option<DrainPlan> {
        if self.pending_lines == 0 {
            return None;
        }

        self.update_mode();

        match self.mode {
            ChunkingMode::Smooth => Some(DrainPlan::Single),
            ChunkingMode::CatchUp => Some(DrainPlan::Batch),
        }
    }

    /// 消费后排空积压计数
    pub(crate) fn drain(&mut self) {
        self.pending_lines = 0;
        self.oldest_chunk_at = None;
    }

    /// 重置策略状态（用于 done/interrupt/begin_round）
    pub(crate) fn reset(&mut self) {
        self.mode = ChunkingMode::Smooth;
        self.pending_lines = 0;
        self.oldest_chunk_at = None;
    }

    /// 根据队列深度和最老行年龄更新模式
    fn update_mode(&mut self) {
        let now = Instant::now();
        let oldest_age = self
            .oldest_chunk_at
            .map(|t| now.duration_since(t))
            .unwrap_or(Duration::ZERO);

        match self.mode {
            ChunkingMode::Smooth => {
                // 进入 CatchUp：满足任一条件
                if self.pending_lines >= self.queue_depth_threshold
                    || oldest_age >= self.oldest_age_threshold
                {
                    self.mode = ChunkingMode::CatchUp;
                }
            }
            ChunkingMode::CatchUp => {
                // 退出 CatchUp：同时满足两个条件
                if self.pending_lines <= self.exit_depth && oldest_age <= self.exit_age {
                    self.mode = ChunkingMode::Smooth;
                }
            }
        }
    }

    /// 当前是否处于 CatchUp 模式（诊断用）
    #[allow(dead_code)]
    fn is_catch_up(&self) -> bool {
        self.mode == ChunkingMode::CatchUp
    }
}

// ── MessagePipeline 上的节流方法 ──

impl super::MessagePipeline {
    /// 检查自适应节流策略，根据流式渲染模式决定是否发射 RebuildAll。
    ///
    /// - Streaming 模式：自适应分块策略（Smooth/CatchUp）
    /// - Block 模式：检测 block_pending_flush 标记
    /// - None 模式：始终返回 None（不触发流式重绘）
    ///
    /// 由 poll_agent() 每帧调用。
    pub(crate) fn check_throttle(&mut self, prefix_len: usize) -> Option<PipelineAction> {
        match self.streaming_mode {
            super::StreamingMode::Streaming => self.check_throttle_streaming(prefix_len),
            super::StreamingMode::Block => self.check_throttle_block(prefix_len),
            super::StreamingMode::None => None,
        }
    }

    fn check_throttle_streaming(&mut self, prefix_len: usize) -> Option<PipelineAction> {
        let plan = self.adaptive_policy.check()?;

        match plan {
            DrainPlan::Single => {
                let now = Instant::now();
                let min_interval = Duration::from_millis(16);
                let should_fire = match self.throttle_last_fire {
                    None => true,
                    Some(last) => now.duration_since(last) >= min_interval,
                };
                if !should_fire {
                    return None;
                }
                self.throttle_last_fire = Some(now);
                self.adaptive_policy.drain();
                Some(self.build_rebuild_all(prefix_len))
            }
            DrainPlan::Batch => {
                self.throttle_last_fire = Some(Instant::now());
                self.adaptive_policy.drain();
                Some(self.build_rebuild_all(prefix_len))
            }
        }
    }

    fn check_throttle_block(&mut self, prefix_len: usize) -> Option<PipelineAction> {
        if self.block_pending_flush {
            self.block_pending_flush = false;
            Some(self.build_rebuild_all(prefix_len))
        } else {
            None
        }
    }
}
