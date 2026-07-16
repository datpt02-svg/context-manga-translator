//! `POST /pipelines` — start a pipeline run as a long-running operation.
//!
//! Returns an `operationId`. Progress + completion flow through SSE
//! (`JobStarted` / `JobProgress` / `JobFinished`). Cancellation goes to
//! `DELETE /operations/{id}`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use axum::Json;
use axum::extract::State;
use koharu_app::pipeline::{
    self, PipelineRunOptions, PipelineSpec, ProgressTick, Scope, WarningTick,
};
use koharu_core::{
    AppEvent, JobFinishedEvent, JobStatus, JobSummary, JobWarningEvent, NodeId, PageId,
    PipelineProgress, PipelineStatus, ReadingOrder, Region, UnlimitedOcrMode,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::routes::operations::{register_cancel, unregister_cancel};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::default().routes(routes!(start_pipeline))
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartPipelineRequest {
    /// Engine ids (`inventory::submit!` ids) to run in order.
    pub steps: Vec<String>,
    /// `None` → whole project, `Some(pages)` → just those pages.
    #[serde(default)]
    pub pages: Option<Vec<PageId>>,
    /// Optional bounding-box hint for inpainter engines (repair-brush).
    #[serde(default)]
    pub region: Option<Region>,
    /// Optional text-node ids for engines that can operate on individual blocks.
    #[serde(default)]
    pub text_node_ids: Option<Vec<NodeId>>,
    #[serde(default)]
    pub target_language: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub default_font: Option<String>,
    #[serde(default)]
    pub reading_order: Option<ReadingOrder>,
    /// `None` = fall back to saved config.
    #[serde(default)]
    pub unlimited_ocr_mode: Option<UnlimitedOcrMode>,
    #[serde(default)]
    pub unlimited_ocr_url: Option<String>,
    /// Custom system prompt for vLLM OCR engine.
    #[serde(default)]
    pub vllm_ocr_system_prompt: Option<String>,
    /// Target language hint for vLLM OCR — replaces `{{ target_language }}` in the prompt.
    #[serde(default)]
    pub vllm_ocr_target_language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartPipelineResponse {
    pub operation_id: String,
}

#[utoipa::path(
    post,
    path = "/pipelines",
    request_body = StartPipelineRequest,
    responses((status = 200, body = StartPipelineResponse))
)]
async fn start_pipeline(
    State(app): State<AppState>,
    Json(req): Json<StartPipelineRequest>,
) -> ApiResult<Json<StartPipelineResponse>> {
    let session = app
        .current_session()
        .ok_or_else(|| ApiError::bad_request("no project open"))?;
    // Validate every step resolves to a registered engine before spawning.
    for id in &req.steps {
        pipeline::Registry::find(id).map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    }

    let app_config = app.config.load();
    let pipeline_config = app_config.pipeline.clone();

    // Full mode: replace OCR engine step with unlimited-ocr
    let mut steps = req.steps.clone();
    let unlimited_ocr_mode = req
        .unlimited_ocr_mode
        .unwrap_or(pipeline_config.unlimited_ocr_mode);
    if unlimited_ocr_mode == UnlimitedOcrMode::Full {
        for step in steps.iter_mut() {
            if *step == "paddle-ocr-vl-1.6" || *step == "manga-ocr" || *step == "mit48px-ocr" {
                *step = "unlimited-ocr".to_string();
            }
        }
    }

    let detector_confidence_threshold = pipeline_config.detector_confidence_threshold;
    let segmenter_binary_threshold = pipeline_config.segmenter_binary_threshold;
    let comic_text_bubble_detector_classes = pipeline_config.comic_text_bubble_detector_classes;

    // Resolve vLLM OCR provider settings from provider `vllm-ocr`.
    let vllm_provider = app_config.providers.iter().find(|p| p.id == "vllm-ocr");
    let vllm_ocr_base_url = vllm_provider.and_then(|p| p.base_url.clone());
    let vllm_ocr_model = vllm_provider.and_then(|p| p.model.clone());
    let vllm_ocr_api_key = vllm_provider
        .and_then(|p| p.api_key.as_ref())
        .map(|s| s.expose().to_owned());
    let vllm_ocr_max_tokens = vllm_provider.and_then(|p| p.max_tokens);
    let vllm_ocr_temperature = vllm_provider.and_then(|p| p.temperature);
    let vllm_ocr_system_prompt = req.vllm_ocr_system_prompt.clone();
    let vllm_ocr_target_language = req.vllm_ocr_target_language.clone();

    // Unlimited-OCR URL: request → saved config → default.
    let unlimited_ocr_url = req
        .unlimited_ocr_url
        .or_else(|| pipeline_config.unlimited_ocr_url.clone());

    // AnyText2 URL: saved config → default.
    let anytext2_url = pipeline_config.anytext2_url.clone();

    let spec = PipelineSpec {
        scope: match req.pages {
            Some(pages) => Scope::Pages(pages),
            None => Scope::WholeProject,
        },
        steps,
        options: PipelineRunOptions {
            target_language: req.target_language,
            system_prompt: req.system_prompt,
            default_font: req.default_font,
            text_node_ids: req.text_node_ids,
            region: req.region,
            reading_order: req.reading_order,
            unlimited_ocr_mode,
            unlimited_ocr_url,
            anytext2_url,
            detector_confidence_threshold,
            segmenter_binary_threshold,
            comic_text_bubble_detector_classes,
            vllm_ocr_base_url,
            vllm_ocr_model,
            vllm_ocr_api_key,
            vllm_ocr_max_tokens,
            vllm_ocr_temperature,
            vllm_ocr_system_prompt,
            vllm_ocr_target_language,
        },
    };

    let operation_id = Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    register_cancel(operation_id.clone(), cancel.clone());
    app.jobs.insert(
        operation_id.clone(),
        JobSummary {
            id: operation_id.clone(),
            kind: "pipeline".to_string(),
            status: JobStatus::Running,
            error: None,
        },
    );
    app.bus.publish(AppEvent::JobStarted {
        id: operation_id.clone(),
        kind: "pipeline".to_string(),
    });

    // Detach the pipeline. Progress writes directly into the jobs registry;
    // clients observe via SSE.
    let app_c = app.clone();
    let session_c = session.clone();
    let op_id_c = operation_id.clone();
    let registry_c = app.registry.clone();
    let runtime_c = app.runtime.clone();
    let llm_c = app.llm.clone();
    let renderer_c = app.renderer.clone();
    let cpu = app.cpu_only();
    let progress_bus = app.bus.clone();
    let progress_op_id = operation_id.clone();
    let progress_sink: pipeline::ProgressSink = Arc::new(move |tick: ProgressTick| {
        progress_bus.publish(AppEvent::JobProgress(PipelineProgress {
            job_id: progress_op_id.clone(),
            status: PipelineStatus::Running,
            step: tick.step,
            current_page: tick.page_index,
            total_pages: tick.total_pages,
            current_step_index: tick.step_index,
            total_steps: tick.total_steps,
            overall_percent: tick.overall_percent,
            detail: tick.detail,
        }));
    });
    let warning_bus = app.bus.clone();
    let warning_op_id = operation_id.clone();
    let warning_sink: pipeline::WarningSink = Arc::new(move |tick: WarningTick| {
        warning_bus.publish(AppEvent::JobWarning(JobWarningEvent {
            job_id: warning_op_id.clone(),
            page_index: tick.page_index,
            total_pages: tick.total_pages,
            step_id: tick.step_id,
            message: tick.message,
        }));
    });
    tokio::spawn(async move {
        let result = pipeline::run(
            session_c,
            registry_c,
            runtime_c,
            cpu,
            llm_c,
            renderer_c,
            spec,
            cancel,
            Some(progress_sink),
            Some(warning_sink),
        )
        .await;
        let (status, error) = match &result {
            Ok(outcome) if outcome.warning_count == 0 => (JobStatus::Completed, None),
            Ok(outcome) => (
                JobStatus::CompletedWithErrors,
                Some(format!(
                    "{} step(s) failed; see warnings for details",
                    outcome.warning_count
                )),
            ),
            Err(e) if e.to_string().contains("cancelled") => (JobStatus::Cancelled, None),
            Err(e) => {
                tracing::warn!(operation_id = %op_id_c, "pipeline run failed: {e:#}");
                (JobStatus::Failed, Some(format!("{e:#}")))
            }
        };
        app_c.jobs.insert(
            op_id_c.clone(),
            JobSummary {
                id: op_id_c.clone(),
                kind: "pipeline".to_string(),
                status,
                error: error.clone(),
            },
        );
        app_c.bus.publish(AppEvent::JobFinished(JobFinishedEvent {
            id: op_id_c.clone(),
            status,
            error,
        }));
        unregister_cancel(&op_id_c);
    });

    Ok(Json(StartPipelineResponse { operation_id }))
}
