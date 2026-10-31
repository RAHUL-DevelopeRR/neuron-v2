"""Summarise a CSV of orders.

Usage: python report.py [--json] ORDERS.csv
"""
import csv
import json
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
    args = argv[1:]
    as_json = "--json" in args
    paths = [arg for arg in args if arg != "--json"]
    if len(paths) != 1:
        print("usage: python report.py ORDERS.csv", file=sys.stderr)
        return 2
    orders = load_orders(paths[0])
    summary = summarise(orders)
    if as_json:
        largest = max(orders, key=lambda order: order["amount"], default=None)
        print(json.dumps({
            "orders": summary["count"],
            "total": round(summary["total"], 2),
            "customers": summary["customers"],
            "largest_order": None if largest is None else {"id": largest["id"], "amount": largest["amount"]},
        }))
        return 0
    print(f"{summary['count']} orders, total {summary['total']:.2f}")
    print("customers: " + ", ".join(summary["customers"]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
