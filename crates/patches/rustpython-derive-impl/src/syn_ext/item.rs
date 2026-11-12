use crate::syn_ext::ident::GetIdent;
use syn::{
    spanned::Spanned, Attribute, Ident, ImplItem, ImplItemFn, Item, ItemFn, ItemMod, Result,
    TraitItem, TraitItemFn,
};

pub trait ItemLike: Spanned {
    fn attrs(&self) -> Result<&[Attribute]>;
    fn attrs_mut(&mut self) -> Result<&mut Vec<Attribute>>;
    fn function_or_method(&self) -> Result<&dyn FunctionLike>;
    fn constant(&self) -> Result<&dyn ConstLike>;

    fn is_type(&self) -> bool;
    fn is_macro(&self) -> bool;
}

impl ItemLike for Item {
    fn attrs(&self) -> Result<&[Attribute]> {
        use syn::Item::*;
        use syn::*;
        let attrs = match self {
            Const(ItemConst { attrs, .. }) => attrs,
            Enum(ItemEnum { attrs, .. }) => attrs,
            ExternCrate(ItemExternCrate { attrs, .. }) => attrs,
            Fn(ItemFn { attrs, .. }) => attrs,
            ForeignMod(ItemForeignMod { attrs, .. }) => attrs,
            Impl(ItemImpl { attrs, .. }) => attrs,
            Macro(ItemMacro { attrs, .. }) => attrs,
            Mod(ItemMod { attrs, .. }) => attrs,
            Static(ItemStatic { attrs, .. }) => attrs,
            Struct(ItemStruct { attrs, .. }) => attrs,
            Trait(ItemTrait { attrs, .. }) => attrs,
            TraitAlias(ItemTraitAlias { attrs, .. }) => attrs,
            Type(ItemType { attrs, .. }) => attrs,
            Union(ItemUnion { attrs, .. }) => attrs,
            Use(ItemUse { attrs, .. }) => attrs,
            other => {
                return Err(Error::new_spanned(
                    other,
                    "this kind of item doesn't have attrs",
                ))
            }
        };
        Ok(attrs)
    }

    fn attrs_mut(&mut self) -> Result<&mut Vec<Attribute>> {
        use syn::Item::*;
        use syn::*;
        let attrs = match self {
            Const(ItemConst { attrs, .. }) => attrs,
            Enum(ItemEnum { attrs, .. }) => attrs,
            ExternCrate(ItemExternCrate { attrs, .. }) => attrs,
            Fn(ItemFn { attrs, .. }) => attrs,
            ForeignMod(ItemForeignMod { attrs, .. }) => attrs,
            Impl(ItemImpl { attrs, .. }) => attrs,
            Macro(ItemMacro { attrs, .. }) => attrs,
            Mod(ItemMod { attrs, .. }) => attrs,
            Static(ItemStatic { attrs, .. }) => attrs,
            Struct(ItemStruct { attrs, .. }) => attrs,
            Trait(ItemTrait { attrs, .. }) => attrs,
            TraitAlias(ItemTraitAlias { attrs, .. }) => attrs,
            Type(ItemType { attrs, .. }) => attrs,
            Union(ItemUnion { attrs, .. }) => attrs,
            Use(ItemUse { attrs, .. }) => attrs,
            other => {
                return Err(Error::new_spanned(
                    other,
                    "this kind of item doesn't have attrs",
                ))
            }
        };
        Ok(attrs)
    }

    fn function_or_method(&self) -> Result<&dyn FunctionLike> {
        match self {
            Item::Fn(f @ syn::ItemFn { .. }) => Ok(f),
            other => Err(syn::Error::new_spanned(
                other,
                "this item is not a function or method",
            )),
        }
    }

    fn constant(&self) -> Result<&dyn ConstLike> {
        match self {
            Item::Const(c @ syn::ItemConst { .. }) => Ok(c),
            other => Err(syn::Error::new_spanned(other, "this item is not a const")),
        }
    }

    fn is_type(&self) -> bool {
        matches!(self, Item::Type(_))
    }
    fn is_macro(&self) -> bool {
        matches!(self, Item::Macro(_))
    }
}

impl ItemLike for ImplItem {
    fn attrs(&self) -> Result<&[Attribute]> {
        use syn::ImplItem::*;
        use syn::*;
        let attrs = match self {
            Const(ImplItemConst { attrs, .. }) => attrs,
            Fn(ImplItemFn { attrs, .. }) => attrs,
            Type(ImplItemType { attrs, .. }) => attrs,
            Macro(ImplItemMacro { attrs, .. }) => attrs,
            other => {
                return Err(Error::new_spanned(
                    other,
                    "this kind of item doesn't have attrs",
                ))
            }
        };
        Ok(attrs)
    }

    fn attrs_mut(&mut self) -> Result<&mut Vec<Attribute>> {
        use syn::ImplItem::*;
        use syn::*;
        let attrs = match self {
            Const(ImplItemConst { attrs, .. }) => attrs,
            Fn(ImplItemFn { attrs, .. }) => attrs,
            Type(ImplItemType { attrs, .. }) => attrs,
            Macro(ImplItemMacro { attrs, .. }) => attrs,
            other => {
                return Err(Error::new_spanned(
                    other,
                    "this kind of item doesn't have attrs",
                ))
            }
        };
        Ok(attrs)
    }

    fn function_or_method(&self) -> Result<&dyn FunctionLike> {
        match self {
            ImplItem::Fn(f @ syn::ImplItemFn { .. }) => Ok(f),
            other => Err(syn::Error::new_spanned(
                other,
                "this item is not a function or method",
            )),
        }
    }

    fn constant(&self) -> Result<&dyn ConstLike> {
        match self {
            ImplItem::Const(c @ syn::ImplItemConst { .. }) => Ok(c),
            other => Err(syn::Error::new_spanned(other, "this item is not a const")),
        }
    }

    fn is_type(&self) -> bool {
        matches!(self, ImplItem::Type(_))
    }
    fn is_macro(&self) -> bool {
        matches!(self, ImplItem::Macro(_))
    }
}

impl ItemLike for TraitItem {
    fn attrs(&self) -> Result<&[Attribute]> {
        use syn::TraitItem::*;
        use syn::*;
        let attrs = match self {
            Const(TraitItemConst { attrs, .. }) => attrs,
            Fn(TraitItemFn { attrs, .. }) => attrs,
            Type(TraitItemType { attrs, .. }) => attrs,
            Macro(TraitItemMacro { attrs, .. }) => attrs,
            other => {
                return Err(Error::new_spanned(
                    other,
                    "this kind of item doesn't have attrs",
                ))
            }
        };
        Ok(attrs)
    }

    fn attrs_mut(&mut self) -> Result<&mut Vec<Attribute>> {
        use syn::TraitItem::*;
        use syn::*;
        let attrs = match self {
            Const(TraitItemConst { attrs, .. }) => attrs,
            Fn(TraitItemFn { attrs, .. }) => attrs,
            Type(TraitItemType { attrs, .. }) => attrs,
            Macro(TraitItemMacro { attrs, .. }) => attrs,
            other => {
                return Err(Error::new_spanned(
                    other,
                    "this kind of impl item doesn't have attrs",
                ))
            }
        };
        Ok(attrs)
    }

    fn function_or_method(&self) -> Result<&dyn FunctionLike> {
        match self {
            TraitItem::Fn(f @ syn::TraitItemFn { .. }) => Ok(f),
            other => Err(syn::Error::new_spanned(
                other,
                "this item is not a function or method",
            )),
        }
    }

    fn constant(&self) -> Result<&dyn ConstLike> {
        match self {
            TraitItem::Const(c @ syn::TraitItemConst { .. }) => Ok(c),
            other => Err(syn::Error::new_spanned(other, "this item is not a const")),
        }
    }

    fn is_type(&self) -> bool {
        matches!(self, TraitItem::Type(_))
    }
    fn is_macro(&self) -> bool {
        matches!(self, TraitItem::Macro(_))
    }
}

pub trait ItemAttrExt: ItemLike {
    fn try_split_attr_mut<F, R>(&mut self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Vec<Attribute>, &mut Self) -> Result<R>,
    {
        let mut attrs = std::mem::take(self.attrs_mut()?);
        let result = f(&mut attrs, self);
        let _temp = std::mem::replace(self.attrs_mut().unwrap(), attrs);
        assert!(
            _temp.is_empty(),
            "attrs changed during replacement. this behavior must be a bug."
        );
        result
    }
}

impl ItemAttrExt for Item {}
impl ItemAttrExt for ImplItem {}
impl ItemAttrExt for TraitItem {}

pub trait ItemModExt {
    fn items(&self) -> Option<&[Item]>;
    fn items_mut(&mut self) -> Option<&mut Vec<Item>>;
}

impl ItemModExt for ItemMod {
    fn items(&self) -> Option<&[Item]> {
        if let Some((_, content)) = self.content.as_ref() {
            Some(content)
        } else {
            None
        }
    }
    fn items_mut(&mut self) -> Option<&mut Vec<Item>> {
        if let Some((_, content)) = self.content.as_mut() {
            Some(content)
        } else {
            None
        }
    }
}

impl GetIdent for Item {
    fn get_ident(&self) -> Option<&Ident> {
        use syn::Item::*;
        use syn::UseTree::*;
        use syn::*;
        #[allow(clippy::collapsible_match)]
        let attrs = match self {
            Const(ItemConst { ident, .. }) => ident,
            Enum(ItemEnum { ident, .. }) => ident,
            ExternCrate(ItemExternCrate { ident, .. }) => ident,
            Fn(ItemFn { sig, .. }) => &sig.ident,
            Impl(ItemImpl { .. }) => unimplemented!(),
            Macro(ItemMacro { ident, .. }) => return ident.as_ref(),
            Mod(ItemMod { ident, .. }) => ident,
            Static(ItemStatic { ident, .. }) => ident,
            Struct(ItemStruct { ident, .. }) => ident,
            Trait(ItemTrait { ident, .. }) => ident,
            TraitAlias(ItemTraitAlias { ident, .. }) => ident,
            Type(ItemType { ident, .. }) => ident,
            Union(ItemUnion { ident, .. }) => ident,
            Use(ItemUse { tree, .. }) => match tree {
                Name(UseName { ident }) => ident,
                _ => return None,
            },
            _ => return None,
        };
        Some(attrs)
    }
}

impl GetIdent for ImplItem {
    fn get_ident(&self) -> Option<&Ident> {
        use syn::ImplItem::*;
        use syn::*;
        let ident = match self {
            Const(ImplItemConst { ident, .. }) => ident,
            Fn(ImplItemFn { sig, .. }) => &sig.ident,
            Type(ImplItemType { ident, .. }) => ident,
            Macro(ImplItemMacro {
                mac: syn::Macro { path, .. },
                ..
            }) => return path.get_ident(),
            _ => return None,
        };
        Some(ident)
    }
}

impl GetIdent for TraitItem {
    fn get_ident(&self) -> Option<&Ident> {
        use syn::TraitItem::*;
        use syn::*;
        let ident = match self {
            Const(TraitItemConst { ident, .. }) => ident,
            Fn(TraitItemFn { sig, .. }) => &sig.ident,
            Type(TraitItemType { ident, .. }) => ident,
            Macro(TraitItemMacro {
                mac: syn::Macro { path, .. },
                ..
            }) => return path.get_ident(),
            _ => return None,
        };
        Some(ident)
    }
}

pub trait FunctionLike: Spanned {
    fn attrs(&self) -> &[Attribute];
    fn attrs_mut(&mut self) -> &mut Vec<Attribute>;
    fn vis(&self) -> &syn::Visibility;

    fn sig(&self) -> &syn::Signature;
    fn block(&self) -> Option<&syn::Block>;
}

impl FunctionLike for ItemFn {
    fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Vec<Attribute> {
        &mut self.attrs
    }
    fn vis(&self) -> &syn::Visibility {
        &self.vis
    }
    fn sig(&self) -> &syn::Signature {
        &self.sig
    }
    fn block(&self) -> Option<&syn::Block> {
        Some(&self.block)
    }
}

impl FunctionLike for ImplItemFn {
    fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Vec<Attribute> {
        &mut self.attrs
    }
    fn vis(&self) -> &syn::Visibility {
        &self.vis
    }
    fn sig(&self) -> &syn::Signature {
        &self.sig
    }
    fn block(&self) -> Option<&syn::Block> {
        Some(&self.block)
    }
}

impl FunctionLike for TraitItemFn {
    fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Vec<Attribute> {
        &mut self.attrs
    }
    fn vis(&self) -> &syn::Visibility {
        &syn::Visibility::Inherited
    }
    fn sig(&self) -> &syn::Signature {
        &self.sig
    }
    fn block(&self) -> Option<&syn::Block> {
        self.default.as_ref()
    }
}

pub trait ConstLike: Spanned {
    fn attrs(&self) -> &[Attribute];
    fn attrs_mut(&mut self) -> &mut Vec<Attribute>;
    fn vis(&self) -> &syn::Visibility;
    fn const_token(&self) -> &syn::token::Const;
    fn ident(&self) -> &syn::Ident;
    fn colon_token(&self) -> &syn::token::Colon;
    fn ty(&self) -> &syn::Type;
}

impl ConstLike for syn::ItemConst {
    fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Vec<Attribute> {
        &mut self.attrs
    }
    fn vis(&self) -> &syn::Visibility {
        &self.vis
    }
    fn const_token(&self) -> &syn::token::Const {
        &self.const_token
    }
    fn ident(&self) -> &syn::Ident {
        &self.ident
    }
    fn colon_token(&self) -> &syn::token::Colon {
        &self.colon_token
    }
    fn ty(&self) -> &syn::Type {
        &self.ty
    }
}

impl ConstLike for syn::ImplItemConst {
    fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Vec<Attribute> {
        &mut self.attrs
    }
    fn vis(&self) -> &syn::Visibility {
        &self.vis
    }
    fn const_token(&self) -> &syn::token::Const {
        &self.const_token
    }
    fn ident(&self) -> &syn::Ident {
        &self.ident
    }
    fn colon_token(&self) -> &syn::token::Colon {
        &self.colon_token
    }
    fn ty(&self) -> &syn::Type {
        &self.ty
    }
}

impl ConstLike for syn::TraitItemConst {
    fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Vec<Attribute> {
        &mut self.attrs
    }
    fn vis(&self) -> &syn::Visibility {
        &syn::Visibility::Inherited
    }
    fn const_token(&self) -> &syn::token::Const {
        &self.const_token
    }
    fn ident(&self) -> &syn::Ident {
        &self.ident
    }
    fn colon_token(&self) -> &syn::token::Colon {
        &self.colon_token
    }
    fn ty(&self) -> &syn::Type {
        &self.ty
    }
}

