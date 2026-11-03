"""Summarise a CSV of orders.

Usage: python report.py ORDERS.csv
"""
import csv
import sys


def load_orders(path):
    with open(path, newline="", encoding="utf-8") as handle:
        return [
            {"id": row["id"], "customer": row["customer"], "amount": float(row["amount"])}
            for row in csv.DictReader(handle)
        ]


def summarise(orders):
    total = sum(order["amount"] for order in orders)
    customers = sorted({order["customer"] for order in orders})
    return {"count": len(orders), "total": total, "customers": customers}


def main(argv):
    if len(argv) != 2:
        print("usage: python report.py ORDERS.csv", file=sys.stderr)
        return 2
    summary = summarise(load_orders(argv[1]))
    print(f"{summary['count']} orders, total {summary['total']:.2f}")
    print("customers: " + ", ".join(summary["customers"]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
