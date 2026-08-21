"""Pydantic schemas for FoxOps APIs."""

from pydantic import BaseModel, Field


class FaultCodeCreate(BaseModel):
    code: str = Field(min_length=1, max_length=80)
    title: str = Field(min_length=1, max_length=200)
    severity: str | None = None
    equipment_type: str = Field(min_length=1, max_length=50)
    equipment_model: str | None = Field(default=None, max_length=80)
    description: str | None = None
    diagnosis_plan: str | None = None
    source_document_id: str | None = Field(default=None, max_length=64)


class FaultCodeUpdate(BaseModel):
    code: str | None = Field(default=None, min_length=1, max_length=80)
    title: str | None = Field(default=None, min_length=1, max_length=200)
    severity: str | None = None
    equipment_type: str | None = Field(default=None, max_length=50)
    equipment_model: str | None = Field(default=None, max_length=80)
    description: str | None = None
    diagnosis_plan: str | None = None
    source_document_id: str | None = Field(default=None, max_length=64)


class FaultCodeResponse(BaseModel):
    id: int
    code: str
    title: str
    severity: str | None
    equipment_type: str
    equipment_model: str | None
    description: str | None
    diagnosis_plan: str | None
    source_document_id: str | None
    created_at: str | None
    updated_at: str | None


class FaultCodeListResponse(BaseModel):
    items: list[FaultCodeResponse]
    total: int
