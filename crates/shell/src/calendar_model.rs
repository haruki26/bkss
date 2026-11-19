use chrono::{Datelike, Local, NaiveDate};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CalendarDay {
    pub date: NaiveDate,
    pub day: u32,
    pub is_today: bool,
    pub is_weekend: bool,
    pub is_other_month: bool,
}

pub fn month_grid(month: NaiveDate, today: NaiveDate) -> [[CalendarDay; 7]; 6] {
    let first = month.with_day(1).unwrap_or(month);
    let start = first - chrono::Duration::days(first.weekday().num_days_from_sunday() as i64);
    std::array::from_fn(|week| {
        std::array::from_fn(|weekday| {
            let date = start + chrono::Duration::days((week * 7 + weekday) as i64);
            CalendarDay {
                date,
                day: date.day(),
                is_today: date == today,
                is_weekend: matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
                is_other_month: date.month() != first.month(),
            }
        })
    })
}

pub fn current_month() -> NaiveDate {
    let today = Local::now().date_naive();
    today.with_day(1).unwrap_or(today)
}

pub fn shift_month(month: NaiveDate, delta: i32) -> NaiveDate {
    let index = month.year() * 12 + month.month0() as i32 + delta;
    let year = index.div_euclid(12);
    let month = index.rem_euclid(12) as u32 + 1;
    NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(month_day_clamp(year, month))
}

fn month_day_clamp(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year.clamp(1, 9999), month.clamp(1, 12), 1).unwrap_or(NaiveDate::MIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_is_sunday_first_six_weeks_and_marks_today() {
        let month = NaiveDate::from_ymd_opt(2024, 2, 1).unwrap();
        let today = NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
        let grid = month_grid(month, today);
        assert_eq!(
            grid[0][0].date,
            NaiveDate::from_ymd_opt(2024, 1, 28).unwrap()
        );
        assert_eq!(grid[4][4].date, today);
        assert!(grid[4][4].is_today);
        assert_eq!(
            grid[5][6].date,
            NaiveDate::from_ymd_opt(2024, 3, 9).unwrap()
        );
        assert!(grid[0][0].is_other_month);
    }

    #[test]
    fn shifts_months_across_year_boundaries() {
        let december = NaiveDate::from_ymd_opt(2024, 12, 1).unwrap();
        assert_eq!(
            shift_month(december, 1),
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()
        );
        assert_eq!(
            shift_month(december, -12),
            NaiveDate::from_ymd_opt(2023, 12, 1).unwrap()
        );
    }
}
