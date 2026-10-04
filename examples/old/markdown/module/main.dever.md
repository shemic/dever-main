# 购物车结算

计算购物车中所有商品的合计金额、优惠金额和最终应付金额。

- 包：`main`
- 公开类型：
  - `Product`
  - `OrderSummary`
- 公开方法：无
- 使用：无

## 商品

保存商品名称、单价和购买数量。

- 类型：`Product`
- 字段：
  - `name: Text`：商品名称
  - `price: Decimal`：商品单价
  - `quantity: Int >= 1`：购买数量，最少为 1

```typescript dever
public type Product {
  name: Text
  price: Decimal
  quantity: Int >= 1
}
```

## 订单结算结果

保存参与结算的商品以及各项金额。

- 类型：`OrderSummary`
- 字段：
  - `items: List<Product>`：参与结算的商品
  - `subtotal: Decimal`：优惠前的商品合计
  - `discount: Decimal`：优惠金额
  - `payable: Decimal`：最终应付金额

```typescript dever
public type OrderSummary {
  items: List<Product>
  subtotal: Decimal
  discount: Decimal
  payable: Decimal
}
```

## 计算单项商品金额

使用商品单价乘以购买数量。

- 函数：`line_total`
- 输入：
  - `product: Product`：需要计算金额的商品
- 输出：
  - `total: Decimal`：该商品的总金额

```typescript dever
line_total(product: Product) (total: Decimal) {
  total = product.price * product.quantity
}
```

## 汇总商品金额

计算商品列表中所有商品的金额总和。

- 函数：`sum_products`
- 输入：
  - `products: List<Product>`：需要汇总的商品列表
- 输出：
  - `subtotal: Decimal`：所有商品的合计金额

```typescript dever
sum_products(products: List<Product>) (subtotal: Decimal) {
  subtotal = sum(line_total, products)
}
```

## 结算订单

根据商品列表和会员应付比例计算订单结果。

- 函数：`checkout`
- 输入：
  - `products: List<Product>`：需要结算的商品列表
  - `member_rate: Decimal`：会员实际应付比例，例如 0.90
- 输出：
  - `summary: OrderSummary`：订单结算结果

```typescript dever
checkout(products: List<Product>, member_rate: Decimal) (summary: OrderSummary) {
  subtotal = sum_products(products)
  payable = decimal.round(subtotal * member_rate, 2)
  discount = subtotal - payable
  summary = OrderSummary {
    items = products
    subtotal = subtotal
    discount = discount
    payable = payable
  }
}
```

## 运行购物车示例

创建商品列表，并以九折会员价格完成结算。

- 函数：`main`
- 输入：无
- 输出：
  - `customer: Text`：客户名称
  - `summary: OrderSummary`：客户的订单结算结果

```typescript dever
main() (customer: Text, summary: OrderSummary) {
  customer = "小明"
  products = [
    Product {
      name = "笔记本"
      price = 12.50
      quantity = 3
    },
    Product {
      name = "签字笔"
      price = 5.00
      quantity = 2
    }
  ]
  summary = checkout(products, 0.90)
}
```

### 执行

```bash
deverc check examples/old/markdown --entry app.main
deverc fmt examples/old/markdown --check
deverc run examples/old/markdown app.main
```

结算结果：

| 项目 | 金额 |
| --- | ---: |
| 商品合计 | 47.50 |
| 优惠金额 | 4.75 |
| 应付金额 | 42.75 |
