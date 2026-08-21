from __future__ import annotations

import pytest
import pytest_asyncio
from fastapi import HTTPException
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from yuxi.foxops.models import (
    FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_SQL,
    FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_STATEMENTS,
    FoxOpsFaultCode,
)
from yuxi.foxops.repositories import FaultCodeRepository
from yuxi.foxops.schemas import FaultCodeCreate, FaultCodeUpdate
from yuxi.foxops.services.fault_code_service import FaultCodeService
from yuxi.storage.postgres.models_business import Base

pytestmark = [pytest.mark.asyncio, pytest.mark.unit]


@pytest_asyncio.fixture()
async def service():
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)
    factory = async_sessionmaker(engine, expire_on_commit=False)
    async with factory() as db:
        yield FaultCodeService(FaultCodeRepository(db))
    await engine.dispose()


async def test_fault_code_model_is_registered_on_business_metadata():
    assert FoxOpsFaultCode.__table__.metadata is Base.metadata
    assert "foxops_fault_code" in Base.metadata.tables


async def test_fault_code_updated_at_trigger_sql_targets_table():
    sql = FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_SQL

    assert "CREATE OR REPLACE FUNCTION foxops_set_updated_at" in sql
    assert "NEW.updated_at = (now() AT TIME ZONE 'UTC')" in sql
    assert "BEFORE UPDATE ON foxops_fault_code" in sql
    assert "EXECUTE FUNCTION foxops_set_updated_at()" in sql


async def test_fault_code_updated_at_trigger_uses_single_command_statements():
    assert len(FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_STATEMENTS) == 3
    assert FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_STATEMENTS[0].strip().startswith("CREATE OR REPLACE FUNCTION")
    assert FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_STATEMENTS[1].strip().startswith("DROP TRIGGER IF EXISTS")
    assert FOXOPS_FAULT_CODE_UPDATED_AT_TRIGGER_STATEMENTS[2].strip().startswith("CREATE TRIGGER")


async def test_create_and_search_fault_code_returns_expected_fields(service):
    created = await service.create_fault_code(
        FaultCodeCreate(
            code="E-4312",
            title="起升变频器过流",
            severity="high",
            equipment_type="QC",
            equipment_model="STS-A",
            description="起升下降时电流异常升高",
            diagnosis_plan="检查起升变频器报警记录、制动器释放状态和电机电缆绝缘。",
            source_document_id="kf_manual_001",
        )
    )

    result = await service.list_fault_codes(q="4312")

    assert created["id"] > 0
    assert created["severity"] == "high"
    assert created["equipment_model"] == "STS-A"
    assert created["diagnosis_plan"] == "检查起升变频器报警记录、制动器释放状态和电机电缆绝缘。"
    assert "status" not in created
    assert "sensitivity" not in created
    assert "version" not in created
    assert created["source_document_id"] == "kf_manual_001"
    assert result["total"] == 1
    assert result["items"][0]["code"] == "E-4312"


async def test_create_allows_generic_and_model_specific_code(service):
    generic = FaultCodeCreate(code="E-1001", title="编码器反馈异常", equipment_type="QC")
    model_specific = FaultCodeCreate(
        code="E-1001",
        title="A 型编码器反馈异常",
        equipment_type="QC",
        equipment_model="STS-A",
    )

    await service.create_fault_code(generic)
    created = await service.create_fault_code(model_specific)

    assert created["equipment_model"] == "STS-A"


async def test_create_rejects_duplicate_code_in_same_equipment_scope(service):
    payload = FaultCodeCreate(code="E-1001", title="编码器反馈异常", equipment_type="QC")
    await service.create_fault_code(payload)

    with pytest.raises(HTTPException) as exc:
        await service.create_fault_code(payload)

    assert exc.value.status_code == 409


async def test_list_model_fault_codes_returns_generic_and_matching_model(service):
    await service.create_fault_code(FaultCodeCreate(code="E-1001", title="通用故障", equipment_type="QC"))
    await service.create_fault_code(
        FaultCodeCreate(code="E-1002", title="A 型故障", equipment_type="QC", equipment_model="STS-A")
    )
    await service.create_fault_code(
        FaultCodeCreate(code="E-1003", title="B 型故障", equipment_type="QC", equipment_model="STS-B")
    )

    result = await service.list_fault_codes(equipment_type="QC", equipment_model="STS-A")

    assert result["total"] == 2
    assert {item["code"] for item in result["items"]} == {"E-1001", "E-1002"}


async def test_create_accepts_unclassified_severity_value(service):
    created = await service.create_fault_code(
        FaultCodeCreate(code="E-1002", title="制动器未释放", severity="pending-level", equipment_type="QC")
    )

    assert created["severity"] == "pending-level"


async def test_update_diagnosis_plan(service):
    created = await service.create_fault_code(FaultCodeCreate(code="E-1004", title="制动器未释放", equipment_type="QC"))

    updated = await service.update_fault_code(
        created["id"], FaultCodeUpdate(diagnosis_plan="检查制动器电源与限位反馈。")
    )

    assert updated["diagnosis_plan"] == "检查制动器电源与限位反馈。"


async def test_delete_fault_code_removes_row(service):
    created = await service.create_fault_code(FaultCodeCreate(code="E-1003", title="限位信号异常", equipment_type="QC"))

    deleted = await service.delete_fault_code(created["id"])
    result = await service.list_fault_codes()

    assert deleted == {"id": created["id"], "deleted": True}
    assert result["total"] == 0
