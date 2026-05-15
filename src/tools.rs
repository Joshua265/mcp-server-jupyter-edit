use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::format::{format_single_cell, llm_format_to_notebook, notebook_to_llm_format};
use crate::io::{read_notebook_file, write_notebook_file};
use crate::notebook::{Cell, CellType};

#[derive(Debug, Deserialize)]
pub struct ReadNotebookRequest {
    pub path: String,
    #[serde(default = "default_limit")]
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

fn default_limit() -> Option<usize> {
    Some(100)
}

pub fn read_notebook(req: ReadNotebookRequest) -> Result<String> {
    let notebook = read_notebook_file(&req.path)?;
    let filename = Path::new(&req.path)
        .file_name()
        .and_then(|n| n.to_str());
    Ok(notebook_to_llm_format(&notebook, filename, req.limit, req.offset))
}

#[derive(Debug, Deserialize)]
pub struct WriteNotebookRequest {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct WriteNotebookResponse {
    pub message: String,
    pub warnings: Vec<String>,
}

pub fn write_notebook(req: WriteNotebookRequest) -> Result<WriteNotebookResponse> {
    let (notebook, warnings) = llm_format_to_notebook(&req.content)?;
    write_notebook_file(&req.path, &notebook)?;

    Ok(WriteNotebookResponse {
        message: format!("Notebook written successfully to: {}", req.path),
        warnings,
    })
}

#[derive(Debug, Deserialize)]
pub struct ListCellsRequest {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct CellInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub cell_type: String,
    pub index: usize,
    pub source_preview: String,
}

pub fn list_cells(req: ListCellsRequest) -> Result<Vec<CellInfo>> {
    let notebook = read_notebook_file(&req.path)?;
    
    Ok(notebook
        .cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let source = cell.source.as_string();
            let preview = if source.len() > 100 {
                format!("{}...", &source[..100])
            } else {
                source
            };
            CellInfo {
                id: cell.id.clone(),
                cell_type: cell.cell_type.to_string().to_lowercase(),
                index,
                source_preview: preview,
            }
        })
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct GetCellRequest {
    pub path: String,
    pub cell_id: String,
}

pub fn get_cell(req: GetCellRequest) -> Result<String> {
    let notebook = read_notebook_file(&req.path)?;
    
    let cell = notebook
        .cells
        .iter()
        .find(|c| c.id.as_ref().map(|id| id.as_str()) == Some(req.cell_id.as_str()))
        .ok_or_else(|| anyhow!("Cell not found with ID: {}", req.cell_id))?;

    Ok(format_single_cell(cell, &notebook))
}

#[derive(Debug, Deserialize)]
pub struct AddCellRequest {
    pub path: String,
    pub cell_type: String,
    pub content: String,
    pub after_cell_id: Option<String>,
    pub after_index: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct AddCellResponse {
    pub message: String,
    pub cell_id: String,
}

pub fn add_cell(req: AddCellRequest) -> Result<AddCellResponse> {
    let cell_type = match req.cell_type.as_str() {
        "code" => CellType::Code,
        "markdown" => CellType::Markdown,
        "raw" => CellType::Raw,
        _ => anyhow::bail!("Invalid cell type: {}. Must be 'code', 'markdown', or 'raw'", req.cell_type),
    };

    let mut notebook = read_notebook_file(&req.path)?;

    let insert_index = if let Some(index) = req.after_index {
        if index > notebook.cells.len() {
            anyhow::bail!("Index {} is out of bounds (max: {})", index, notebook.cells.len());
        }
        index
    } else if let Some(after_id) = &req.after_cell_id {
        let pos = notebook
            .cells
            .iter()
            .position(|c| c.id.as_ref().map(|id| id.as_str()) == Some(after_id.as_str()))
            .ok_or_else(|| anyhow!("Cell to insert after not found: {}", after_id))?;
        pos + 1
    } else {
        notebook.cells.len()
    };

    let new_cell = Cell::new(cell_type, req.content);
    let cell_id = new_cell.id.clone().unwrap();
    
    notebook.cells.insert(insert_index, new_cell);
    write_notebook_file(&req.path, &notebook)?;

    Ok(AddCellResponse {
        message: format!("Cell added successfully with ID: {}", cell_id),
        cell_id,
    })
}

#[derive(Debug, Deserialize)]
pub struct UpdateCellRequest {
    pub path: String,
    pub cell_id: String,
    pub content: String,
}

pub fn update_cell(req: UpdateCellRequest) -> Result<String> {
    let mut notebook = read_notebook_file(&req.path)?;

    let cell = notebook
        .cells
        .iter_mut()
        .find(|c| c.id.as_ref().map(|id| id.as_str()) == Some(req.cell_id.as_str()))
        .ok_or_else(|| anyhow!("Cell not found with ID: {}", req.cell_id))?;

    cell.source = crate::notebook::CellSource::Single(req.content);
    write_notebook_file(&req.path, &notebook)?;

    Ok(format!("Cell updated successfully: {}", req.cell_id))
}

#[derive(Debug, Deserialize)]
pub struct DeleteCellRequest {
    pub path: String,
    pub cell_id: String,
}

pub fn delete_cell(req: DeleteCellRequest) -> Result<String> {
    let mut notebook = read_notebook_file(&req.path)?;

    let index = notebook
        .cells
        .iter()
        .position(|c| c.id.as_ref().map(|id| id.as_str()) == Some(req.cell_id.as_str()))
        .ok_or_else(|| anyhow!("Cell not found with ID: {}", req.cell_id))?;

    let removed = notebook.cells.remove(index);
    write_notebook_file(&req.path, &notebook)?;

    Ok(format!(
        "Cell deleted successfully: {} (type: {})",
        req.cell_id,
        removed.cell_type.to_string().to_lowercase()
    ))
}
