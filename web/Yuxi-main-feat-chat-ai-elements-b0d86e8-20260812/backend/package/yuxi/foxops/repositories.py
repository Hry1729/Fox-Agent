"""Database access for FoxOps business tables."""

from sqlalchemy import func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from yuxi.foxops.models import FoxOpsFaultCode
from yuxi.foxops.schemas import FaultCodeCreate, FaultCodeUpdate


class FaultCodeRepository:
    def __init__(self, db: AsyncSession):
        self.db = db

    async def list(
        self,
        *,
        q: str | None = None,
        equipment_type: str | None = None,
        equipment_model: str | None = None,
        limit: int = 20,
        offset: int = 0,
    ) -> tuple[list[FoxOpsFaultCode], int]:
        filters = []
        if q:
            pattern = f"%{q}%"
            filters.append(or_(FoxOpsFaultCode.code.ilike(pattern), FoxOpsFaultCode.title.ilike(pattern)))
        if equipment_type:
            filters.append(FoxOpsFaultCode.equipment_type == equipment_type)
            if equipment_model:
                filters.append(
                    or_(FoxOpsFaultCode.equipment_model.is_(None), FoxOpsFaultCode.equipment_model == equipment_model)
                )
            else:
                filters.append(FoxOpsFaultCode.equipment_model.is_(None))

        stmt = select(FoxOpsFaultCode)
        count_stmt = select(func.count()).select_from(FoxOpsFaultCode)
        if filters:
            stmt = stmt.where(*filters)
            count_stmt = count_stmt.where(*filters)

        stmt = stmt.order_by(FoxOpsFaultCode.updated_at.desc(), FoxOpsFaultCode.id.desc()).offset(offset).limit(limit)
        result = await self.db.execute(stmt)
        total = await self.db.scalar(count_stmt)
        return list(result.scalars().all()), int(total or 0)

    async def get_by_id(self, fault_code_id: int) -> FoxOpsFaultCode | None:
        return await self.db.get(FoxOpsFaultCode, fault_code_id)

    async def get_by_code(self, code: str) -> FoxOpsFaultCode | None:
        stmt = select(FoxOpsFaultCode).where(FoxOpsFaultCode.code == code).order_by(FoxOpsFaultCode.id)
        result = await self.db.execute(stmt)
        return result.scalars().first()

    async def get_by_scope(
        self,
        *,
        code: str,
        equipment_type: str,
        equipment_model: str | None,
    ) -> FoxOpsFaultCode | None:
        filters = [
            FoxOpsFaultCode.code == code,
            FoxOpsFaultCode.equipment_type == equipment_type,
        ]
        if equipment_model is None:
            filters.append(FoxOpsFaultCode.equipment_model.is_(None))
        else:
            filters.append(FoxOpsFaultCode.equipment_model == equipment_model)
        result = await self.db.execute(select(FoxOpsFaultCode).where(*filters))
        return result.scalar_one_or_none()

    async def create(self, data: FaultCodeCreate) -> FoxOpsFaultCode:
        item = FoxOpsFaultCode(**data.model_dump())
        self.db.add(item)
        await self.db.commit()
        await self.db.refresh(item)
        return item

    async def update(self, item: FoxOpsFaultCode, data: FaultCodeUpdate) -> FoxOpsFaultCode:
        for field, value in data.model_dump(exclude_unset=True).items():
            setattr(item, field, value)
        await self.db.commit()
        await self.db.refresh(item)
        return item

    async def delete(self, item: FoxOpsFaultCode) -> None:
        await self.db.delete(item)
        await self.db.commit()
