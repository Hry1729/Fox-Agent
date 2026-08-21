"""FoxOps business tables."""

from typing import Any

from sqlalchemy import Column, DateTime, Index, Integer, String, Text, text
from yuxi.storage.postgres.models_business import Base
from yuxi.utils.datetime_utils import format_utc_datetime, utc_now_naive


FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_STATEMENTS = (
    """
    CREATE OR REPLACE FUNCTION foxops_set_updated_at()
    RETURNS trigger AS $$
    BEGIN
        NEW.updated_at = (now() AT TIME ZONE 'UTC');
        RETURN NEW;
    END;
    $$ LANGUAGE plpgsql;
    """,
    "DROP TRIGGER IF EXISTS trg_foxops_fault_code_updated_at ON foxops_fault_code;",
    """
    CREATE TRIGGER trg_foxops_fault_code_updated_at
    BEFORE UPDATE ON foxops_fault_code
    FOR EACH ROW
    EXECUTE FUNCTION foxops_set_updated_at();
    """,
)
FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_SQL = "\n".join(FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_STATEMENTS)


class FoxOpsFaultCode(Base):
    """Fault code master data for FoxOps maintenance workflows."""

    __tablename__ = "foxops_fault_code"
    __table_args__ = (
        Index(
            "uq_foxops_fault_code_generic_scope",
            "equipment_type",
            "code",
            unique=True,
            postgresql_where=text("equipment_model IS NULL"),
            sqlite_where=text("equipment_model IS NULL"),
        ),
        Index(
            "uq_foxops_fault_code_model_scope",
            "equipment_type",
            "equipment_model",
            "code",
            unique=True,
            postgresql_where=text("equipment_model IS NOT NULL"),
            sqlite_where=text("equipment_model IS NOT NULL"),
        ),
    )

    id = Column(Integer, primary_key=True, autoincrement=True)
    code = Column(String(80), nullable=False, index=True)
    title = Column(String(200), nullable=False)
    severity = Column(String(20), nullable=True)
    equipment_type = Column(String(50), nullable=False, index=True)
    equipment_model = Column(String(80), nullable=True, index=True)
    description = Column(Text, nullable=True)
    diagnosis_plan = Column(Text, nullable=True)
    source_document_id = Column(String(64), nullable=True, index=True)
    created_at = Column(DateTime, default=utc_now_naive)
    updated_at = Column(DateTime, default=utc_now_naive, onupdate=utc_now_naive)

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "code": self.code,
            "title": self.title,
            "severity": self.severity,
            "equipment_type": self.equipment_type,
            "equipment_model": self.equipment_model,
            "description": self.description,
            "diagnosis_plan": self.diagnosis_plan,
            "source_document_id": self.source_document_id,
            "created_at": format_utc_datetime(self.created_at),
            "updated_at": format_utc_datetime(self.updated_at),
        }
