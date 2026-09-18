Detects columns declared as `real` (`float4`), `double precision` (`float8`), or `float`. IEEE 754 floating-point types suffer from precision issues — for example, `0.1 + 0.2 ≠ 0.3`. For money, quantities, measurements, or any domain where exact decimal values matter, `numeric`/`decimal` is the correct choice.

**Example** (bad):
```sql
CREATE TABLE products (price double precision NOT NULL);
```

**Fix**:
```sql
CREATE TABLE products (price numeric(10,2) NOT NULL);
```
