from fastapi import APIRouter, Depends, Query
from sqlalchemy.ext.asyncio import AsyncSession

from server.utils.auth_middleware import get_admin_user, get_db, get_required_user
from yuxi.foxops.repositories import FaultCodeRepository
from yuxi.foxops.schemas import FaultCodeCreate, FaultCodeListResponse, FaultCodeResponse, FaultCodeUpdate
from yuxi.foxops.services.fault_code_service import FaultCodeService
from yuxi.storage.postgres.models_business import User

foxops = APIRouter(prefix="/foxops", tags=["foxops"])


def _fault_code_service(db: AsyncSession) -> FaultCodeService:
    return FaultCodeService(FaultCodeRepository(db))


@foxops.get("/fault-codes", response_model=FaultCodeListResponse)
async def list_fault_codes(
    q: str | None = Query(default=None),
    equipment_type: str | None = Query(default=None),
    equipment_model: str | None = Query(default=None),
    limit: int = Query(default=20, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
    current_user: User = Depends(get_required_user),
    db: AsyncSession = Depends(get_db),
):
    service = _fault_code_service(db)
    return await service.list_fault_codes(
        q=q,
        equipment_type=equipment_type,
        equipment_model=equipment_model,
        limit=limit,
        offset=offset,
    )


@foxops.get("/fault-codes/{code}", response_model=FaultCodeResponse)
async def get_fault_code(
    code: str,
    current_user: User = Depends(get_required_user),
    db: AsyncSession = Depends(get_db),
):
    service = _fault_code_service(db)
    return await service.get_fault_code(code)


@foxops.post("/fault-codes", response_model=FaultCodeResponse)
async def create_fault_code(
    payload: FaultCodeCreate,
    current_user: User = Depends(get_admin_user),
    db: AsyncSession = Depends(get_db),
):
    service = _fault_code_service(db)
    return await service.create_fault_code(payload)


@foxops.patch("/fault-codes/{fault_code_id}", response_model=FaultCodeResponse)
async def update_fault_code(
    fault_code_id: int,
    payload: FaultCodeUpdate,
    current_user: User = Depends(get_admin_user),
    db: AsyncSession = Depends(get_db),
):
    service = _fault_code_service(db)
    return await service.update_fault_code(fault_code_id, payload)


@foxops.delete("/fault-codes/{fault_code_id}")
async def delete_fault_code(
    fault_code_id: int,
    current_user: User = Depends(get_admin_user),
    db: AsyncSession = Depends(get_db),
):
    service = _fault_code_service(db)
    return await service.delete_fault_code(fault_code_id)
