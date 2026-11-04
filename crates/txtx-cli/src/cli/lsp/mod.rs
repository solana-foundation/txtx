mod native_bridge;

use self::native_bridge::LspNativeBridge;
use std::sync::mpsc;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
use tower_lsp::{LspService, Server};
use txtx_core::kit::channel::unbounded;
use txtx_core::kit::types::diagnostics::{Diagnostic as TxtxDiagnostic, DiagnosticLevel};

pub async fn run_lsp() -> Result<(), String> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (notification_tx, notification_rx) = unbounded();
    let (request_tx, request_rx) = unbounded();
    let (response_tx, response_rx) = mpsc::channel();
    std::thread::spawn(move || {
        txtx_system_kit::nestable_block_on(native_bridge::start_language_server(
            notification_rx,
            request_rx,
            response_tx,
        ));
    });

    let (service, socket) = LspService::new(|client| {
        LspNativeBridge::new(client, notification_tx, request_tx, response_rx)
    });
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}

pub fn clarity_diagnostics_to_tower_lsp_type(
    diagnostics: &mut [TxtxDiagnostic],
) -> Vec<tower_lsp::lsp_types::Diagnostic> {
    let mut dst = vec![];
    for d in diagnostics.iter_mut() {
        dst.push(clarity_diagnostic_to_tower_lsp_type(d));
    }
    dst
}

pub fn clarity_diagnostic_to_tower_lsp_type(
    diagnostic: &TxtxDiagnostic,
) -> tower_lsp::lsp_types::Diagnostic {
    let range = match &diagnostic.span {
        None => Range::default(),
        Some(span) => Range {
            start: Position { line: span.line_start - 1, character: span.column_start - 1 },
            end: Position { line: span.line_end - 1, character: span.column_end },
        },
    };
    // TODO(lgalabru): add hint for contracts not found errors
    Diagnostic {
        range,
        severity: match diagnostic.level {
            DiagnosticLevel::Error => Some(DiagnosticSeverity::ERROR),
            DiagnosticLevel::Warning => Some(DiagnosticSeverity::WARNING),
            DiagnosticLevel::Note => Some(DiagnosticSeverity::INFORMATION),
        },
        code: None,
        code_description: None,
        source: Some("clarity".to_string()),
        message: diagnostic.message.clone(),
        related_information: None,
        tags: None,
        data: None,
    }
}
