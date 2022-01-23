use itertools::Itertools;
use ordered_float::NotNan;
use term_table::{row::Row, table_cell::TableCell};

use crate::{
    config::Config,
    item_type::{ItemTypeAveraged, SystemMarketsItemData},
    order_ext::OrderIterExt,
    requests::to_not_nan,
};

use super::help::{averages, best_buy_volume_from_sell_to_sell, max_avg_price};

pub fn get_good_items_sell_sell(
    pairs: Vec<SystemMarketsItemData>,
    config: &Config,
) -> Vec<PairCalculatedDataSellSell> {
    pairs
        .into_iter()
        .map(|x| {
            let src_mkt_orders = x.source.orders.clone();
            let src_mkt_volume = src_mkt_orders.iter().sell_order_volume();

            let dst_mkt_orders = x.destination.orders.clone();
            let dst_mkt_volume: i32 = dst_mkt_orders.iter().sell_order_volume();

            let src_avgs = averages(config, &x.source.history);
            let dst_avgs = averages(config, &x.destination.history);

            let dst_lowest_sell_order = dst_mkt_orders
                .iter()
                .filter(|x| !x.is_buy_order)
                .map(|x| to_not_nan(x.price))
                .min()
                .map(|x| *x);
            let dst_max_price = max_avg_price(config, &x.destination.history);

            // average can be none only if there's no history in dest
            // in this case we make history
            let dest_sell_price = dst_max_price.unwrap_or_else(|| {
                log::debug!(
                    "Item {} ({}) doesn't have any history in destination.",
                    x.desc.name,
                    x.desc.type_id
                );
                src_avgs.average.unwrap_or(0.) * 1.3
            });
            let dest_sell_price =
                dst_lowest_sell_order.map_or(dest_sell_price, |x| x.min(dest_sell_price));

            let max_buy_vol = (dst_avgs.volume * config.rcmnd_fill_days)
                .max(1.)
                .min(src_mkt_volume as f64)
                .floor() as i32;

            let (buy_from_src_price, buy_from_src_volume) =
                (!x.source.orders.iter().any(|x| !x.is_buy_order))
                    .then_some((
                        src_avgs.highest.or_else(|| {
                            log::debug!(
                                "Item {} ({}) doesn't have any history in source.",
                                x.desc.name,
                                x.desc.type_id
                            );
                            None
                        })?,
                        max_buy_vol,
                    ))
                    .unwrap_or_else(|| {
                        let (price, volume) = best_buy_volume_from_sell_to_sell(
                            x.source.orders.as_slice(),
                            max_buy_vol,
                            dest_sell_price,
                            config.broker_fee_source,
                            config.broker_fee_destination,
                            config.sales_tax,
                        );
                        (price, volume)
                    });
            if buy_from_src_volume == 0 {
                return None;
            }

            let buy_price = buy_from_src_price * (1. + config.broker_fee_source);
            let expenses = buy_price
                + x.desc.volume.unwrap() as f64 * config.freight_cost_iskm3
                + buy_price * config.freight_cost_collateral_percent;

            let sell_price =
                dest_sell_price * (1. - config.broker_fee_destination - config.sales_tax);

            let margin = (sell_price - expenses) / expenses;

            let rough_profit = (sell_price - expenses) * buy_from_src_volume as f64;

            let filled_for_days =
                (dst_avgs.volume > 0.).then(|| 1. / dst_avgs.volume * dst_mkt_volume as f64);

            Some(PairCalculatedDataSellSell {
                market: x,
                margin,
                rough_profit,
                market_dest_volume: dst_mkt_volume,
                recommend_buy: buy_from_src_volume,
                expenses,
                sell_price,
                filled_for_days,
                src_buy_price: buy_from_src_price,
                dest_min_sell_price: dest_sell_price,
                market_src_volume: src_mkt_volume,
                src_avgs,
                dst_avgs,
            })
        })
        .flatten()
        .filter(|x| x.margin > config.margin_cutoff)
        .filter(|x| {
            x.src_avgs.volume > config.min_src_volume && x.dst_avgs.volume > config.min_dst_volume
        })
        .filter(|x| {
            if let Some(filled_for_days) = x.filled_for_days {
                filled_for_days < config.max_filled_for_days_cutoff
            } else {
                true
            }
        })
        .sorted_unstable_by_key(|x| NotNan::new(-x.rough_profit).unwrap())
        .take(config.items_take)
        .collect::<Vec<_>>()
}

pub fn make_table_sell_sell<'a, 'b>(
    good_items: &'a [PairCalculatedDataSellSell],
    name_length: usize,
) -> Vec<Row<'b>> {
    let rows = std::iter::once(Row::new(vec![
        TableCell::new("id"),
        TableCell::new("item name"),
        TableCell::new("src prc"),
        TableCell::new("dst prc"),
        TableCell::new("expenses"),
        TableCell::new("sell prc"),
        TableCell::new("margin"),
        TableCell::new("vlm src"),
        TableCell::new("vlm dst"),
        TableCell::new("mkt src"),
        TableCell::new("mkt dst"),
        TableCell::new("rough prft"),
        TableCell::new("rcmnd vlm"),
        TableCell::new("fld fr dy"),
    ]))
    .chain(good_items.iter().map(|it| {
        let short_name =
            it.market.desc.name[..(name_length.min(it.market.desc.name.len()))].to_owned();
        Row::new(vec![
            TableCell::new(format!("{}", it.market.desc.type_id)),
            TableCell::new(short_name),
            TableCell::new(format!("{:.2}", it.src_buy_price)),
            TableCell::new(format!("{:.2}", it.dest_min_sell_price)),
            TableCell::new(format!("{:.2}", it.expenses)),
            TableCell::new(format!("{:.2}", it.sell_price)),
            TableCell::new(format!("{:.2}", it.margin)),
            TableCell::new(format!("{:.2}", it.src_avgs.volume)),
            TableCell::new(format!("{:.2}", it.dst_avgs.volume)),
            TableCell::new(format!("{:.2}", it.market_src_volume)),
            TableCell::new(format!("{:.2}", it.market_dest_volume)),
            TableCell::new(format!("{:.2}", it.rough_profit)),
            TableCell::new(format!("{}", it.recommend_buy)),
            TableCell::new(
                it.filled_for_days
                    .map_or("N/A".to_string(), |x| format!("{:.2}", x)),
            ),
        ])
    }))
    .collect::<Vec<_>>();
    rows
}

pub struct PairCalculatedDataSellSell {
    pub market: SystemMarketsItemData,
    pub margin: f64,
    pub rough_profit: f64,
    pub market_dest_volume: i32,
    pub recommend_buy: i32,
    pub expenses: f64,
    pub sell_price: f64,
    pub filled_for_days: Option<f64>,
    pub src_buy_price: f64,
    pub dest_min_sell_price: f64,
    pub src_avgs: ItemTypeAveraged,
    pub dst_avgs: ItemTypeAveraged,
    pub market_src_volume: i32,
}
