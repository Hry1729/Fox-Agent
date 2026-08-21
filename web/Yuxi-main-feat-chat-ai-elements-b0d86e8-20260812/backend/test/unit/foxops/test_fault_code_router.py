from __future__ import annotations

import pytest
import pytest_asyncio
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from server.routers.foxops_router import foxops
from server.utils.auth_middleware import get_admin_user, get_db, get_required_user
from yuxi.storage.postgres.models_business import Base, Department, User

pytestmark = [pytest.mark.asyncio, pytest.mark.unit]


@pytest_asyncio.fixture()
async def app_context():
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)
    factory = async_sessionmaker(engine, expire_on_commit=False)
    async with factory() as db:
        department = Department(name="FoxOps")
        admin = User(
            username="Admin",
            uid="foxops-admin",
            password_hash="$argon2id$placeholder",
            role="admin",
            department=department,
        )
        user = User(
            username="User",
            uid="foxops-user",
            password_hash="$argon2id$placeholder",
            role="user",
            department=department,
        )
        db.add_all([department, admin, user])
        await db.commit()
        await db.refresh(admin)
        await db.refresh(user)

        async def override_db():
            yield db

        app = FastAPI()
        app.include_router(foxops, prefix="/api")
        app.dependency_overrides[get_db] = override_db
        app.dependency_overrides[get_required_user] = lambda: user
        app.dependency_overrides[get_admin_user] = lambda: admin
        yield app, admin, user
    await engine.dispose()


async def test_fault_code_router_allows_admin_crud_and_user_read(app_context):
    app, _admin, _user = app_context
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        created = await client.post(
            "/api/foxops/fault-codes",
            json={
                "code": "E-4312",
                "title": "起升变频器过流",
                "severity": "high",
                "equipment_type": "QC",
                "equipment_model": "STS-A",
                "diagnosis_plan": "检查起升变频器报警记录、制动器释放状态和电机电缆绝缘。",
            },
        )
        listed = await client.get("/api/foxops/fault-codes", params={"q": "4312"})
        detail = await client.get("/api/foxops/fault-codes/E-4312")

    assert created.status_code == 200
    assert created.json()["code"] == "E-4312"
    assert created.json()["equipment_model"] == "STS-A"
    assert created.json()["diagnosis_plan"] == "检查起升变频器报警记录、制动器释放状态和电机电缆绝缘。"
    assert "status" not in created.json()
    assert listed.status_code == 200
    assert listed.json()["total"] == 1
    assert detail.status_code == 200
    assert detail.json()["title"] == "起升变频器过流"


async def test_fault_code_router_filters_generic_and_model_specific_codes(app_context):
    app, _admin, _user = app_context

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        assert (
            await client.post(
                "/api/foxops/fault-codes",
                json={"code": "E-1001", "title": "通用故障", "equipment_type": "QC"},
            )
        ).status_code == 200
        assert (
            await client.post(
                "/api/foxops/fault-codes",
                json={
                    "code": "E-1002",
                    "title": "A 型故障",
                    "equipment_type": "QC",
                    "equipment_model": "STS-A",
                },
            )
        ).status_code == 200
        assert (
            await client.post(
                "/api/foxops/fault-codes",
                json={
                    "code": "E-1003",
                    "title": "B 型故障",
                    "equipment_type": "QC",
                    "equipment_model": "STS-B",
                },
            )
        ).status_code == 200
        listed = await client.get(
            "/api/foxops/fault-codes",
            params={"equipment_type": "QC", "equipment_model": "STS-A"},
        )

    assert listed.status_code == 200
    assert listed.json()["total"] == 2
    assert {item["code"] for item in listed.json()["items"]} == {"E-1001", "E-1002"}
