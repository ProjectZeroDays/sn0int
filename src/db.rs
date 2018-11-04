use errors::*;

use diesel;
use diesel::expression::SqlLiteral;
use diesel::expression::sql_literal::sql;
use diesel::sql_types::Bool;
use diesel::prelude::*;
use models::*;
use schema::*;
use paths;
use migrations;
use worker;


pub struct Database {
    name: String,
    db: SqliteConnection,
}

impl Database {
    pub fn establish<I: Into<String>>(name: I) -> Result<Database> {
        // TODO: enforce safe name for database
        let name = name.into();

        let path = paths::data_dir()?.join(name.clone() + ".db");
        let path = path.into_os_string().into_string()
            .map_err(|_| format_err!("Failed to convert db path to utf-8"))?;

        let db = worker::spawn_fn("Connecting to database", || {
            let db = SqliteConnection::establish(&path)
                .context("Failed to connect to database")?;
            migrations::run(&db)
                .context("Failed to run migrations")?;
            db.execute("PRAGMA foreign_keys = ON")
                .context("Failed to enforce foreign keys")?;
            Ok(db)
        }, false)?;

        Ok(Database {
            name,
            db,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn db(&self) -> &SqliteConnection {
        &self.db
    }

    /// Returns true if we didn't have this value yet
    pub fn insert_generic(&self, object: &Insert) -> Result<(bool, i32)> {
        match object {
            Insert::Domain(object) => self.insert_struct(&NewDomain {
                value: &object.value,
            }),
            Insert::Subdomain(object) => self.insert_struct(&NewSubdomain {
                domain_id: object.domain_id,
                value: &object.value,
                resolvable: object.resolvable,
            }),
            Insert::IpAddr(object) => self.insert_struct(&NewIpAddr {
                family: &object.family,
                value: &object.value,
                continent: object.continent.as_ref(),
                continent_code: object.continent_code.as_ref(),
                country: object.country.as_ref(),
                country_code: object.country_code.as_ref(),
                city: object.city.as_ref(),
                longitude: object.longitude,
                latitude: object.latitude,
                asn: object.asn,
                as_org: object.as_org.as_ref(),
            }),
            Insert::SubdomainIpAddr(object) => self.insert_subdomain_ipaddr_struct(&NewSubdomainIpAddr {
                subdomain_id: object.subdomain_id,
                ip_addr_id: object.ip_addr_id,
            }),
            Insert::Url(object) => self.insert_struct(&NewUrl {
                subdomain_id: object.subdomain_id,
                value: &object.value,
                status: object.status,
                body: object.body.as_ref(),
                online: object.online,
                title: object.title.as_ref(),
                redirect: object.redirect.as_ref(),
            }),
            Insert::Email(object) => self.insert_struct(&NewEmail {
                value: &object.value,
                valid: object.valid,
            }),
        }
    }

    /// Returns true if we didn't have this value yet
    /// TODO: return changeset if an update has been triggered
    pub fn insert_struct<T: InsertableStruct<M>, M: Model>(&self, obj: &T) -> Result<(bool, i32)> {
        if let Some(existing) = M::get_opt(self, obj.value())? {
            let update = obj.upsert(&existing);
            if update.is_dirty() {
                update.apply(&self)?;
                // TODO: this should return a changeset
                Ok((true, existing.id()))
            } else {
                Ok((false, existing.id()))
            }
        } else {
            obj.insert(&self)?;
            let id = M::get_id(self, obj.value())?;
            Ok((true, id))
        }
    }

    pub fn insert_domain(&self, domain: &str) -> Result<(bool, i32)> {
        let new_domain = NewDomain {
            value: domain,
        };

        self.insert_struct(&new_domain)
    }

    /// Returns true if we didn't have this value yet
    pub fn insert_subdomain(&self, subdomain: &str, domain: &str) -> Result<(bool, i32)> {
        let domain_id = match Domain::get_id_opt(self, domain)? {
            Some(domain_id) => domain_id,
            None => self.insert_domain(domain)?.1,
        };

        let new_subdomain = NewSubdomain {
            domain_id,
            value: &subdomain,
            resolvable: None,
        };

        self.insert_struct(&new_subdomain)
    }

    pub fn insert_ipaddr(&self, family: &str, ipaddr: &str) -> Result<(bool, i32)> {
        // TODO: maybe check if valid
        let new_ipaddr = NewIpAddr {
            family: &family,
            value: &ipaddr,
            continent: None,
            continent_code: None,
            country: None,
            country_code: None,
            city: None,
            longitude: None,
            latitude: None,
            asn: None,
            as_org: None,
        };

        self.insert_struct(&new_ipaddr)
    }

    pub fn insert_subdomain_ipaddr(&self, subdomain_id: i32, ip_addr_id: i32) -> Result<(bool, i32)> {
        self.insert_subdomain_ipaddr_struct(&NewSubdomainIpAddr {
            subdomain_id,
            ip_addr_id,
        })
    }

    pub fn insert_subdomain_ipaddr_struct(&self, subdomain_ipaddr: &NewSubdomainIpAddr) -> Result<(bool, i32)> {
        if let Some(subdomain_ipaddr_id) = SubdomainIpAddr::get_id_opt(self, &(subdomain_ipaddr.subdomain_id, subdomain_ipaddr.ip_addr_id))? {
            Ok((false, subdomain_ipaddr_id))
        } else {
            diesel::insert_into(subdomain_ipaddrs::table)
                .values(subdomain_ipaddr)
                .execute(&self.db)?;
            let id = SubdomainIpAddr::get_id(self, &(subdomain_ipaddr.subdomain_id, subdomain_ipaddr.ip_addr_id))?;
            Ok((true, id))
        }
    }

    pub fn insert_email(&self, email: &str) -> Result<(bool, i32)> {
        let new_email = NewEmail {
            value: email,
            valid: None,
        };

        self.insert_struct(&new_email)
    }

    //

    pub fn update_generic(&self, object: &Update) -> Result<i32> {
        match object {
            Update::Subdomain(object) => self.update_subdomain(object),
            Update::IpAddr(object) => self.update_ipaddr(object),
            Update::Url(object) => self.update_url(object),
            Update::Email(object) => self.update_email(object),
        }
    }

    pub fn update_subdomain(&self, subdomain: &SubdomainUpdate) -> Result<i32> {
        use schema::subdomains::columns::*;
        diesel::update(subdomains::table.filter(id.eq(subdomain.id)))
            .set(subdomain)
            .execute(&self.db)?;
        Ok(subdomain.id)
    }

    pub fn update_ipaddr(&self, ipaddr: &IpAddrUpdate) -> Result<i32> {
        use schema::ipaddrs::columns::*;
        diesel::update(ipaddrs::table.filter(id.eq(ipaddr.id)))
            .set(ipaddr)
            .execute(&self.db)?;
        Ok(ipaddr.id)
    }

    pub fn update_url(&self, url: &UrlUpdate) -> Result<i32> {
        use schema::urls::columns::*;
        diesel::update(urls::table.filter(id.eq(url.id)))
            .set(url)
            .execute(&self.db)?;
        Ok(url.id)
    }

    pub fn update_email(&self, email: &EmailUpdate) -> Result<i32> {
        use schema::emails::columns::*;
        diesel::update(emails::table.filter(id.eq(email.id)))
            .set(email)
            .execute(&self.db)?;
        Ok(email.id)
    }

    //

    pub fn list<T: Model>(&self) -> Result<Vec<T>> {
        T::list(self)
    }

    pub fn filter<T: Model>(&self, filter: &Filter) -> Result<Vec<T>> {
        T::filter(self, filter)
    }

    pub fn scope<T: Scopable>(&self, filter: &Filter) -> Result<usize> {
        T::scope(self, filter)
    }

    pub fn noscope<T: Scopable>(&self, filter: &Filter) -> Result<usize> {
        T::noscope(self, filter)
    }

    pub fn delete<T: Model>(&self, filter: &Filter) -> Result<usize> {
        T::delete(self, filter)
    }
}

#[derive(Debug, PartialEq)]
pub struct Filter {
    query: String,
}

impl Filter {
    pub fn new<I: Into<String>>(query: I) -> Filter {
        Filter {
            query: query.into(),
        }
    }

    fn escape(value: &str) -> String {
        let mut out = String::from("'");
        for c in value.chars() {
            match c {
                '\'' => out.push_str("''"),
                c => out.push(c),
            }
        }
        out.push('\'');
        out
    }

    pub fn parse(mut args: &[String]) -> Result<Filter> {
        debug!("Parsing query: {:?}", args);

        if args.is_empty() {
            bail!("Filter condition is required");
        }

        if args[0].to_lowercase() == "where" {
            args = &args[1..];
        } else {
            bail!("Filter must begin with WHERE");
        }

        let mut query = String::new();

        let mut expect_value = false;

        for arg in args {
            if let Some(idx) = arg.find('=') {
                if idx != 0 {
                    let (key, value) = arg.split_at(idx);
                    query += &format!(" {} = {}", key, Self::escape(&value[1..]));
                    continue;
                }
            }

            if expect_value {
                query.push(' ');
                query.push_str(&Self::escape(arg));
                expect_value = false;
            } else {
                if ["=", "!=", "like"].contains(&arg.to_lowercase().as_str()) {
                    expect_value = true;
                }

                query += &format!(" {}", arg);
            }
        }
        debug!("Parsed query: {:?}", query);

        Ok(Filter::new(query))
    }

    pub fn parse_optional(args: &[String]) -> Result<Filter> {
        debug!("Parsing optional query: {:?}", args);

        if args.is_empty() {
            debug!("Using filter with no condition");
            return Ok(Filter::new("1"));
        }

        Self::parse(args)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn sql(&self) -> SqlLiteral<Bool> {
        sql::<Bool>(&self.query)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_simple() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value=1".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value = '1'"));
    }

    #[test]
    fn test_filter_str1() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value=abc".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value = 'abc'"));
    }

    #[test]
    fn test_filter_str2() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value".to_string(),
                                     "=".to_string(),
                                     "asdf".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value = 'asdf'"));
    }

    #[test]
    fn test_filter_and() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value".to_string(),
                                     "=".to_string(),
                                     "foobar".to_string(),
                                     "and".to_string(),
                                     "id".to_string(),
                                     "=".to_string(),
                                     "1".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value = 'foobar' and id = '1'"));
    }

    #[test]
    fn test_filter_like() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value".to_string(),
                                     "like".to_string(),
                                     "%foobar".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value like '%foobar'"));
    }

    #[test]
    fn test_filter_backslash1() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value=\\".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value = '\\'"));
    }

    #[test]
    fn test_filter_backslash2() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value".to_string(),
                                     "=".to_string(),
                                     "\\".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value = '\\'"));
    }

    #[test]
    fn test_filter_quote1() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value=a'b".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value = 'a''b'"));
    }

    #[test]
    fn test_filter_quote2() {
        let filter = Filter::parse(&["where".to_string(),
                                     "value".to_string(),
                                     "=".to_string(),
                                     "a'b".to_string(),
                                    ]).unwrap();
        assert_eq!(filter, Filter::new(" value = 'a''b'"));
    }
}
