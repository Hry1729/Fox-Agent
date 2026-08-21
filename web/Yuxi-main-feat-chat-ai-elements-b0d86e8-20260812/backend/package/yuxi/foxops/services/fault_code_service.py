"""Fault code use cases."""

from yuxi.foxops.repositories import FaultCodeRepository
from yuxi.foxops.schemas import FaultCodeCreate, FaultCodeUpdate
from fastapi import HTTPException


class FaultCodeService:
    def __init__(self, repo: FaultCodeRepository):
        self.repo = repo

    async def list_fault_codes(
        self,
        *,
        q: str | None = None,
        equipment_type: str | None = None,
        equipment_model: str | None = None,
        limit: int = 20,
        offset: int = 0,
    ) -> dict:
        items, total = await self.repo.list(
            q=q,
            equipment_type=equipment_type,
            equipment_model=equipment_model,
            limit=limit,
            offset=offset,
        )
        return {"items": [item.to_dict() for item in items], "total": total}

    async def get_fault_code(self, code: str) -> dict:
        item = await self.repo.get_by_code(code)
        if not item:
            raise HTTPException(status_code=404, detail="Fault code not found")
        return item.to_dict()

    async def create_fault_code(self, data: FaultCodeCreate) -> dict:
        if await self.repo.get_by_scope(
            code=data.code,
            equipment_type=data.equipment_type,
            equipment_model=data.equipment_model,
        ):
            raise HTTPException(status_code=409, detail="Fault code already exists")
        item = await self.repo.create(data)
        return item.to_dict()

    async def update_fault_code(self, fault_code_id: int, data: FaultCodeUpdate) -> dict:
        item = await self.repo.get_by_id(fault_code_id)
        if not item:
            raise HTTPException(status_code=404, detail="Fault code not found")
        updates = data.model_dump(exclude_unset=True)
        target_code = updates.get("code", item.code)
        target_equipment_type = updates.get("equipment_type", item.equipment_type)
        target_equipment_model = updates.get("equipment_model", item.equipment_model)
        if (
            target_code != item.code
            or target_equipment_type != item.equipment_type
            or target_equipment_model != item.equipment_model
        ):
            existing = await self.repo.get_by_scope(
                code=target_code,
                equipment_type=target_equipment_type,
                equipment_model=target_equipment_model,
            )
            if existing and existing.id != item.id:
                raise HTTPException(status_code=409, detail="Fault code already exists")
        item = await self.repo.update(item, data)
        return item.to_dict()

    async def delete_fault_code(self, fault_code_id: int) -> dict:
        item = await self.repo.get_by_id(fault_code_id)
        if not item:
            raise HTTPException(status_code=404, detail="Fault code not found")
        deleted_id = item.id
        await self.repo.delete(item)
        return {"id": deleted_id, "deleted": True}
